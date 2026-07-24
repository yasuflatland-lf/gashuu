use super::*;
use gashuu_core::{spread_at, MockPageSource, PageEntry};
use std::io::Cursor;

fn tiny_png() -> Vec<u8> {
    let img = image::RgbaImage::from_pixel(2, 3, image::Rgba([9, 9, 9, 255]));
    let mut bytes = Vec::new();
    img.write_to(&mut Cursor::new(&mut bytes), image::ImageFormat::Png)
        .unwrap();
    bytes
}

fn mock_with(pages: usize) -> Arc<dyn PageSource> {
    let mut mock = MockPageSource::new();
    mock.expect_list_pages()
        .returning(move || vec![PageEntry { name: "p".into() }; pages]);
    mock.expect_read_bytes().returning(|_| Ok(tiny_png()));
    Arc::new(mock)
}

/// Build a Double-mode state (Standalone cover, Ltr) via `from_settings`,
/// since the mode fields are private.
fn double_state() -> ViewerState {
    ViewerState::from_settings(&Settings {
        spread_mode: SpreadMode::Double,
        cover_mode: CoverMode::Standalone,
        reading_direction: ReadingDirection::Ltr,
        ..Default::default()
    })
}

#[test]
fn empty_state_shows_nothing() {
    let state = ViewerState::new();
    assert_eq!(state.page_count(), 0);
    assert_eq!(state.index(), 0);
    assert!(state.decode_current_spread().is_none());
    assert_eq!(state.status_content().kind, StatusKind::NoFolder);
}

#[test]
fn empty_folder_status_distinguishes_from_no_folder() {
    let mut state = ViewerState::new();
    state.set_source(mock_with(0));
    assert_eq!(state.status_content().kind, StatusKind::NoImages);
    assert!(state.decode_current_spread().is_none());
}

#[test]
fn next_advances_and_clamps_at_last_page() {
    let mut state = ViewerState::new();
    state.set_source(mock_with(3));
    assert!(state.apply(NavAction::Next));
    assert_eq!(state.index(), 1);
    assert!(state.apply(NavAction::Next));
    assert_eq!(state.index(), 2);
    assert!(!state.apply(NavAction::Next)); // clamped, no move
    assert_eq!(state.index(), 2);
}

#[test]
fn prev_clamps_at_first_page() {
    let mut state = ViewerState::new();
    state.set_source(mock_with(3));
    state.apply(NavAction::Next);
    assert!(state.apply(NavAction::Prev));
    assert_eq!(state.index(), 0);
    assert!(!state.apply(NavAction::Prev)); // clamped at 0
    assert_eq!(state.index(), 0);
}

#[test]
fn single_page_clamps_both_directions() {
    let mut state = ViewerState::new();
    state.set_source(mock_with(1));
    assert!(!state.apply(NavAction::Next));
    assert_eq!(state.index(), 0);
    assert!(!state.apply(NavAction::Prev));
    assert_eq!(state.index(), 0);
}

#[test]
fn set_source_resets_index_to_zero() {
    let mut state = ViewerState::new();
    state.set_source(mock_with(5));
    state.apply(NavAction::Next);
    state.apply(NavAction::Next);
    assert_eq!(state.index(), 2);
    state.set_source(mock_with(3));
    assert_eq!(state.index(), 0);
    assert_eq!(state.page_count(), 3);
}

#[test]
fn decode_current_spread_returns_current_page() {
    let mut state = ViewerState::new();
    state.set_source(mock_with(2));
    let spread = state.decode_current_spread().unwrap().unwrap();
    let leading = spread.leading;
    assert_eq!((leading.width(), leading.height()), (2, 3));
    assert_eq!(leading.rgba().len(), 2 * 3 * 4);
    // Single mode: no trailing page.
    assert!(spread.trailing.is_none());
}

#[test]
fn decode_current_spread_propagates_source_error() {
    let mut state = ViewerState::new();
    let mut mock = MockPageSource::new();
    mock.expect_list_pages()
        .returning(|| vec![PageEntry { name: "p".into() }; 1]);
    mock.expect_read_bytes()
        .returning(|_| Err(CoreError::IndexOutOfRange { index: 0, len: 0 }));
    state.set_source(Arc::new(mock));
    assert!(matches!(state.decode_current_spread(), Some(Err(_))));
}

#[test]
fn status_text_is_one_based() {
    let mut state = ViewerState::new();
    state.set_source(mock_with(100));
    let c = state.status_content();
    assert_eq!(c.kind, StatusKind::Pages);
    assert_eq!(c.pages, "1 / 100");
    assert_eq!(c.spread, SpreadMode::Single);
    assert_eq!(c.direction, ReadingDirection::Ltr);
    state.apply(NavAction::Next);
    assert_eq!(state.status_content().pages, "2 / 100");
}

#[test]
fn status_text_at_last_page() {
    let mut state = ViewerState::new();
    state.set_source(mock_with(3));
    state.apply(NavAction::Next);
    state.apply(NavAction::Next);
    let c = state.status_content();
    assert_eq!(c.pages, "3 / 3");
    assert_eq!(c.spread, SpreadMode::Single);
}

#[test]
fn with_cache_config_stores_values() {
    let state = ViewerState::with_cache_config(CacheConfig::new(7, 1));
    assert_eq!(state.cache_config().capacity(), 7);
    assert_eq!(state.cache_config().radius(), 1);
}

#[test]
fn with_cache_config_defaults_to_single_standalone_ltr() {
    let state = ViewerState::with_cache_config(CacheConfig::new(7, 1));
    assert_eq!(state.spread_mode(), SpreadMode::Single);
    assert_eq!(state.cover_mode(), CoverMode::Standalone);
    assert_eq!(state.reading_direction(), ReadingDirection::Ltr);
}

#[test]
fn from_settings_copies_all_modes_and_cache_config() {
    let state = ViewerState::from_settings(&Settings {
        cache_capacity: 11,
        prefetch_radius: 2,
        spread_mode: SpreadMode::Double,
        cover_mode: CoverMode::Paired,
        reading_direction: ReadingDirection::Rtl,
        ..Default::default()
    });
    assert_eq!(state.cache_config().capacity(), 11);
    assert_eq!(state.cache_config().radius(), 2);
    assert_eq!(state.spread_mode(), SpreadMode::Double);
    assert_eq!(state.cover_mode(), CoverMode::Paired);
    assert_eq!(state.reading_direction(), ReadingDirection::Rtl);
}

// ---- Double-mode (Standalone cover) navigation -------------------------

#[test]
fn double_standalone_navigation_advances_by_spread() {
    // 6 pages, Standalone cover: {0}{1,2}{3,4}{5}.
    let mut state = double_state();
    state.set_source(mock_with(6));
    assert_eq!(state.index(), 0);

    assert!(state.apply(NavAction::Next));
    assert_eq!(state.index(), 1);
    assert!(state.apply(NavAction::Next));
    assert_eq!(state.index(), 3);
    assert!(state.apply(NavAction::Next));
    assert_eq!(state.index(), 5);
    assert!(!state.apply(NavAction::Next)); // clamp at last
    assert_eq!(state.index(), 5);

    // And back down the spreads.
    assert!(state.apply(NavAction::Prev));
    assert_eq!(state.index(), 3);
    assert!(state.apply(NavAction::Prev));
    assert_eq!(state.index(), 1);
    assert!(state.apply(NavAction::Prev));
    assert_eq!(state.index(), 0);
    assert!(!state.apply(NavAction::Prev)); // clamp at start
    assert_eq!(state.index(), 0);
}

#[test]
fn double_standalone_spread_has_trailing_for_pairs_only() {
    // 6 pages, Standalone cover: cover (0) and last odd (5) stand alone;
    // {1,2} and {3,4} have trailing pages.
    let mut state = double_state();
    state.set_source(mock_with(6));

    // Cover page 0: no trailing.
    let cover = state.decode_current_spread().unwrap().unwrap();
    assert!(cover.trailing.is_none());

    // {1,2}: trailing present.
    state.apply(NavAction::Next);
    assert_eq!(state.index(), 1);
    assert!(state
        .decode_current_spread()
        .unwrap()
        .unwrap()
        .trailing
        .is_some());

    // {3,4}: trailing present.
    state.apply(NavAction::Next);
    assert_eq!(state.index(), 3);
    assert!(state
        .decode_current_spread()
        .unwrap()
        .unwrap()
        .trailing
        .is_some());

    // {5}: last odd page stands alone, no trailing.
    state.apply(NavAction::Next);
    assert_eq!(state.index(), 5);
    assert!(state
        .decode_current_spread()
        .unwrap()
        .unwrap()
        .trailing
        .is_none());
}

// ---- Toggles -----------------------------------------------------------

#[test]
fn toggle_spread_flips_mode_and_normalizes_index() {
    // Single mode at the last page (index 5 of 6).
    let mut state = ViewerState::new();
    state.set_source(mock_with(6));
    for _ in 0..5 {
        state.apply(NavAction::Next);
    }
    assert_eq!(state.index(), 5);
    assert_eq!(state.spread_mode(), SpreadMode::Single);

    // Flip to Double: default cover is Standalone, so index 5 (last odd) is
    // already a valid leading and stays put.
    assert!(state.toggle_spread());
    assert_eq!(state.spread_mode(), SpreadMode::Double);
    assert_eq!(state.index(), 5);

    // Cycle to Auto: default viewport aspect is 1.0 -> resolves to Single, so
    // index 5 is still a valid leading and stays put.
    assert!(state.toggle_spread());
    assert_eq!(state.spread_mode(), SpreadMode::Auto);
    assert_eq!(state.index(), 5);

    // Cycle back to Single.
    assert!(state.toggle_spread());
    assert_eq!(state.spread_mode(), SpreadMode::Single);
    assert_eq!(state.index(), 5);
}

#[test]
fn toggle_cover_flips_and_renormalizes() {
    // Double / Standalone at index 5 of 6.
    let mut state = double_state();
    state.set_source(mock_with(6));
    for _ in 0..3 {
        state.apply(NavAction::Next);
    }
    assert_eq!(state.index(), 5);
    assert_eq!(state.cover_mode(), CoverMode::Standalone);

    // Standalone -> Paired: pairs start even, so index 5 normalizes down to
    // the even pair start 4 ({4,5}).
    assert!(state.toggle_cover());
    assert_eq!(state.cover_mode(), CoverMode::Paired);
    assert_eq!(state.index(), 4);

    // Paired -> Standalone again: page 4 (even>0) normalizes to its pair
    // start 3 ({3,4}).
    assert!(state.toggle_cover());
    assert_eq!(state.cover_mode(), CoverMode::Standalone);
    assert_eq!(state.index(), 3);
}

#[test]
fn toggle_reading_direction_flips_and_leaves_index() {
    let mut state = double_state();
    state.set_source(mock_with(6));
    state.apply(NavAction::Next);
    assert_eq!(state.index(), 1);
    assert_eq!(state.reading_direction(), ReadingDirection::Ltr);

    assert!(state.toggle_reading_direction());
    assert_eq!(state.reading_direction(), ReadingDirection::Rtl);
    assert_eq!(state.index(), 1); // pairing unaffected

    assert!(state.toggle_reading_direction());
    assert_eq!(state.reading_direction(), ReadingDirection::Ltr);
    assert_eq!(state.index(), 1);
}

#[test]
fn toggles_are_noop_safe_with_no_folder() {
    // Toggling with no source must not panic and must leave index at 0.
    let mut state = ViewerState::new();
    assert!(state.toggle_spread());
    assert_eq!(state.index(), 0);
    assert!(state.toggle_cover());
    assert_eq!(state.index(), 0);
    assert!(state.toggle_reading_direction());
    assert_eq!(state.index(), 0);
}

// ---- status_text double form -------------------------------------------

#[test]
fn status_text_double_form_shows_range_and_label() {
    // Double / Standalone at index 1 of 6: {1,2} -> "2-3 / 6".
    let mut state = double_state();
    state.set_source(mock_with(6));
    state.apply(NavAction::Next);
    assert_eq!(state.index(), 1);
    let c = state.status_content();
    assert_eq!(c.kind, StatusKind::Pages);
    assert_eq!(c.pages, "2\u{2013}3 / 6");
    assert_eq!(c.spread, SpreadMode::Double);
    assert_eq!(c.direction, ReadingDirection::Ltr);
}

#[test]
fn status_text_double_standalone_cover_is_single_form() {
    // Cover page in Double mode renders as a single page number.
    let mut state = double_state();
    state.set_source(mock_with(6));
    let c = state.status_content();
    assert_eq!(c.pages, "1 / 6");
    assert_eq!(c.spread, SpreadMode::Double);
}

#[test]
fn status_text_reflects_rtl_label() {
    let mut state = ViewerState::from_settings(&Settings {
        spread_mode: SpreadMode::Double,
        cover_mode: CoverMode::Standalone,
        reading_direction: ReadingDirection::Rtl,
        ..Default::default()
    });
    state.set_source(mock_with(6));
    state.apply(NavAction::Next);
    let c = state.status_content();
    assert_eq!(c.pages, "2\u{2013}3 / 6");
    assert_eq!(c.direction, ReadingDirection::Rtl);
}

// ---- Trailing-page decode failure fallback (FIX 4/5) --------------------

#[test]
fn decode_current_spread_degrades_to_leading_on_trailing_decode_error() {
    // 3 pages, Double / Standalone: {0}{1,2}. Page 2 (trailing of {1,2}) is made to
    // fail decode; the spread must degrade to leading-only with a marker.
    let mut state = ViewerState::from_settings(&Settings {
        spread_mode: SpreadMode::Double,
        ..Default::default()
    });
    let mut mock = MockPageSource::new();
    mock.expect_list_pages()
        .returning(|| vec![PageEntry { name: "p".into() }; 3]);
    mock.expect_read_bytes().returning(|idx| {
        if idx == 2 {
            Err(CoreError::IndexOutOfRange { index: 2, len: 3 })
        } else {
            Ok(tiny_png())
        }
    });
    state.set_source(Arc::new(mock));

    // Advance to the {1,2} spread (leading = 1).
    assert!(state.apply(NavAction::Next));
    assert_eq!(state.index(), 1);

    let images = state.decode_current_spread().unwrap().unwrap();
    assert!(images.trailing.is_none(), "trailing should drop on error");
    assert_eq!(images.failed_trailing_page, Some(2));
    assert_eq!(
        (images.leading.width(), images.leading.height()),
        (2, 3),
        "leading page must still decode"
    );
}

// ---- Double/Paired navigation honors stored cover_mode (FIX 6) ---------

#[test]
fn double_paired_navigation_steps_by_two_and_clamps() {
    // 5 pages, Paired cover: {0,1}{2,3}{4}. apply() must honor the stored
    // cover_mode, stepping leading 0->2->4 forward and 4->2->0 back.
    let mut state = ViewerState::from_settings(&Settings {
        spread_mode: SpreadMode::Double,
        cover_mode: CoverMode::Paired,
        ..Default::default()
    });
    state.set_source(mock_with(5));
    assert_eq!(state.index(), 0);

    assert!(state.apply(NavAction::Next));
    assert_eq!(state.index(), 2);
    assert!(state.apply(NavAction::Next));
    assert_eq!(state.index(), 4);
    assert!(!state.apply(NavAction::Next)); // clamp at last even
    assert_eq!(state.index(), 4);

    assert!(state.apply(NavAction::Prev));
    assert_eq!(state.index(), 2);
    assert!(state.apply(NavAction::Prev));
    assert_eq!(state.index(), 0);
    assert!(!state.apply(NavAction::Prev)); // clamp at 0
    assert_eq!(state.index(), 0);
}

// ---- toggle_spread from Double/Paired preserves the visible page (FIX 7)

#[test]
fn toggle_spread_from_double_paired_keeps_index() {
    // 6 pages, Paired cover, advanced to {2,3} (index 2). Toggle to Auto: default
    // viewport aspect 1.0 resolves Auto to Single, so index 2 stays a valid leading.
    let mut state = ViewerState::from_settings(&Settings {
        spread_mode: SpreadMode::Double,
        cover_mode: CoverMode::Paired,
        ..Default::default()
    });
    state.set_source(mock_with(6));
    assert!(state.apply(NavAction::Next));
    assert_eq!(state.index(), 2);

    assert!(state.toggle_spread());
    assert_eq!(state.spread_mode(), SpreadMode::Auto);
    assert_eq!(state.index(), 2);
}

// ---- Auto spread mode (PR4a): resolved from viewport aspect -------------

/// Build an Auto-mode state (Standalone cover, Ltr) via `from_settings`.
fn auto_state() -> ViewerState {
    ViewerState::from_settings(&Settings {
        spread_mode: SpreadMode::Auto,
        cover_mode: CoverMode::Standalone,
        reading_direction: ReadingDirection::Ltr,
        ..Default::default()
    })
}

#[test]
fn auto_portrait_navigates_double() {
    // Portrait viewport (aspect < 1) => Auto resolves to Double. Default
    // Standalone cover: {0}{1,2}{3,4}{5}; navigation steps 0->1->3->5.
    let mut state = auto_state();
    state.set_viewport_size(900.0, 1200.0);
    state.set_source(mock_with(6));

    // Cover (page 0) stands alone.
    assert!(state
        .decode_current_spread()
        .unwrap()
        .unwrap()
        .trailing
        .is_none());

    assert!(state.apply(NavAction::Next));
    assert_eq!(state.index(), 1);
    assert!(state
        .decode_current_spread()
        .unwrap()
        .unwrap()
        .trailing
        .is_some());
    assert!(state.apply(NavAction::Next));
    assert_eq!(state.index(), 3);
    assert!(state
        .decode_current_spread()
        .unwrap()
        .unwrap()
        .trailing
        .is_some());
    assert!(state.apply(NavAction::Next));
    assert_eq!(state.index(), 5);
    assert!(state
        .decode_current_spread()
        .unwrap()
        .unwrap()
        .trailing
        .is_none());
}

#[test]
fn auto_landscape_navigates_single() {
    // Landscape viewport (aspect > 1) => Auto resolves to Single: every page
    // stands alone and navigation steps by 1.
    let mut state = auto_state();
    state.set_viewport_size(1600.0, 900.0);
    state.set_source(mock_with(5));

    assert!(state
        .decode_current_spread()
        .unwrap()
        .unwrap()
        .trailing
        .is_none());

    assert!(state.apply(NavAction::Next));
    assert_eq!(state.index(), 1);
    assert!(state
        .decode_current_spread()
        .unwrap()
        .unwrap()
        .trailing
        .is_none());
    assert!(state.apply(NavAction::Next));
    assert_eq!(state.index(), 2);
    assert!(state
        .decode_current_spread()
        .unwrap()
        .unwrap()
        .trailing
        .is_none());
}

#[test]
fn set_viewport_size_reports_flip_and_renormalizes() {
    // Auto, index 1. Default aspect 1.0 => Single; landscape stays Single, portrait
    // flips to Double, back to landscape flips Single — each flip keeps index 1 visible.
    let mut state = auto_state();
    // Default aspect 1.0 already resolves Auto to Single; widening to
    // landscape stays Single, so this reports no flip.
    assert!(!state.set_viewport_size(1600.0, 900.0));
    state.set_source(mock_with(6));
    assert!(state.apply(NavAction::Next));
    assert_eq!(state.index(), 1);

    // Landscape -> portrait: Single -> Double flips; index 1 ({1,2}) is a valid
    // Standalone Double leading.
    assert!(state.set_viewport_size(900.0, 1200.0));
    assert_eq!(state.index(), 1);

    // Portrait -> landscape: Double -> Single flips again; index 1 stays a
    // valid Single leading, page stays visible.
    assert!(state.set_viewport_size(1600.0, 900.0));
    assert_eq!(state.index(), 1);
}

#[test]
fn set_viewport_size_no_flip_when_not_auto() {
    // Fixed Double mode ignores the viewport aspect: a large aspect change
    // never flips the effective layout, so set_viewport_size returns false.
    let mut state = double_state();
    state.set_source(mock_with(6));
    state.apply(NavAction::Next);
    let before = state.index();

    assert!(!state.set_viewport_size(900.0, 1200.0)); // portrait, but mode is Double
    assert_eq!(state.index(), before);
    assert!(!state.set_viewport_size(1600.0, 900.0)); // landscape, still Double
    assert_eq!(state.index(), before);
}

#[test]
fn toggle_spread_cycles_single_double_auto() {
    // Single -> Double -> Auto -> Single, each transition keeps the visible
    // page on screen (index normalized). Default viewport 1.0 => Auto=Single.
    let mut state = ViewerState::new();
    state.set_source(mock_with(6));
    assert_eq!(state.spread_mode(), SpreadMode::Single);

    assert!(state.toggle_spread());
    assert_eq!(state.spread_mode(), SpreadMode::Double);

    assert!(state.toggle_spread());
    assert_eq!(state.spread_mode(), SpreadMode::Auto);

    assert!(state.toggle_spread());
    assert_eq!(state.spread_mode(), SpreadMode::Single);
}

#[test]
fn toggle_into_auto_resolves_with_current_viewport() {
    // Portrait viewport then cycle into Auto: spread resolves to Double.
    let mut state = ViewerState::new();
    state.set_viewport_size(900.0, 1200.0);
    state.set_source(mock_with(6));
    state.apply(NavAction::Next); // index 1 ({1,2} once Double)
    state.toggle_spread(); // Single -> Double
    state.toggle_spread(); // Double -> Auto
    assert_eq!(state.spread_mode(), SpreadMode::Auto);
    assert!(state
        .decode_current_spread()
        .unwrap()
        .unwrap()
        .trailing
        .is_some());

    // Landscape viewport then cycle into Auto: spread resolves to Single.
    let mut state = ViewerState::new();
    state.set_viewport_size(1600.0, 900.0);
    state.set_source(mock_with(6));
    state.apply(NavAction::Next); // index 1, stands alone once Single
    state.toggle_spread(); // Single -> Double
    state.toggle_spread(); // Double -> Auto
    assert_eq!(state.spread_mode(), SpreadMode::Auto);
    assert!(state
        .decode_current_spread()
        .unwrap()
        .unwrap()
        .trailing
        .is_none());
}

#[test]
fn status_text_auto_label() {
    // Auto + portrait => "auto" label and a page RANGE (resolved Double).
    let mut state = auto_state();
    state.set_viewport_size(900.0, 1200.0);
    state.set_source(mock_with(6));
    state.apply(NavAction::Next);
    assert_eq!(state.index(), 1);
    let c = state.status_content();
    assert_eq!(c.pages, "2\u{2013}3 / 6");
    assert_eq!(c.spread, SpreadMode::Auto);

    // Auto + landscape => "auto" label and a single page number (Single).
    let mut state = auto_state();
    state.set_viewport_size(1600.0, 900.0);
    state.set_source(mock_with(6));
    state.apply(NavAction::Next);
    assert_eq!(state.index(), 1);
    assert_eq!(state.status_content().pages, "2 / 6");
}

#[test]
fn set_viewport_size_flip_moves_index_via_normalize() {
    // Auto + Paired. Landscape => Single (index 1 valid); portrait => Double/Paired
    // makes pairs start even, so normalize rounds index 1 down to 0 (flip must MOVE).
    let mut state = ViewerState::from_settings(&Settings {
        spread_mode: SpreadMode::Auto,
        cover_mode: CoverMode::Paired,
        ..Default::default()
    });
    // Default aspect 1.0 resolves Auto to Single; go landscape => stays Single.
    assert!(!state.set_viewport_size(1600.0, 900.0));
    state.set_source(mock_with(6));
    assert!(state.apply(NavAction::Next));
    assert_eq!(state.index(), 1);

    // Landscape -> portrait: Single -> Double/Paired flips; index 1 normalizes
    // to the even pair start 0.
    assert!(state.set_viewport_size(900.0, 1200.0));
    assert_eq!(state.index(), 0);
}

#[test]
fn toggle_spread_renormalize_moves_index() {
    // Standalone Single at index 2. Toggle to Double: index 2 (even > 0) is an
    // invalid Standalone leading, so normalize re-anchors to pair start 1 (must MOVE).
    let mut state = ViewerState::new();
    state.set_source(mock_with(6));
    state.apply(NavAction::Next);
    state.apply(NavAction::Next);
    assert_eq!(state.index(), 2);
    assert_eq!(state.spread_mode(), SpreadMode::Single);

    assert!(state.toggle_spread());
    assert_eq!(state.spread_mode(), SpreadMode::Double);
    assert_eq!(state.index(), 1);
}

#[test]
fn auto_portrait_with_paired_cover_navigates_double() {
    // Auto + Paired + portrait => Double. 5 pages Paired: {0,1}{2,3}{4};
    // navigation steps leading 0->2->4.
    let mut state = ViewerState::from_settings(&Settings {
        spread_mode: SpreadMode::Auto,
        cover_mode: CoverMode::Paired,
        ..Default::default()
    });
    state.set_viewport_size(900.0, 1200.0);
    state.set_source(mock_with(5));

    // Cover paired with page 1: trailing present.
    assert!(state
        .decode_current_spread()
        .unwrap()
        .unwrap()
        .trailing
        .is_some());

    assert!(state.apply(NavAction::Next));
    assert_eq!(state.index(), 2);
    assert!(state
        .decode_current_spread()
        .unwrap()
        .unwrap()
        .trailing
        .is_some());
    assert!(state.apply(NavAction::Next));
    assert_eq!(state.index(), 4);
    // Last page (4) stands alone.
    assert!(state
        .decode_current_spread()
        .unwrap()
        .unwrap()
        .trailing
        .is_none());
    assert!(!state.apply(NavAction::Next)); // clamp at last
    assert_eq!(state.index(), 4);
}

#[test]
fn set_viewport_size_degenerate_inputs_do_not_panic() {
    // Degenerate sizes must not panic and must not flip from the default 1.0
    // aspect (=> Single); after sanitizing, the stored aspect stays 1.0.
    let mut state = auto_state();
    state.set_source(mock_with(6));

    assert!(!state.set_viewport_size(0.0, 0.0));
    assert!(!state.set_viewport_size(f32::NAN, f32::NAN));
    // Still resolves to Single (aspect stayed 1.0): every page stands alone.
    state.apply(NavAction::Next);
    assert!(state
        .decode_current_spread()
        .unwrap()
        .unwrap()
        .trailing
        .is_none());
}

// ---- open_path dispatch (PR6) -------------------------------------------

#[test]
fn open_path_nonexistent_returns_err() {
    // open must return Err for a missing path (dispatch + error propagation). No
    // tempfile/zip dev-deps here; ZipSource correctness lives in gashuu-core tests.
    let mut state = ViewerState::new();
    let result = state.open_path(std::path::Path::new("/nonexistent_path_pr6_test"));
    assert!(
        result.is_err(),
        "open_path must return Err for a missing path"
    );
    // State must stay clean (no source installed) when open_path errors.
    assert_eq!(state.page_count(), 0);
    assert_eq!(state.index(), 0);
    assert!(state.decode_current_spread().is_none());
}

#[test]
fn last_open_skipped_is_zero_on_fresh_state() {
    // A freshly constructed ViewerState has no open in progress, so
    // last_open_skipped must start at zero.
    assert_eq!(ViewerState::new().last_open_skipped(), 0);
    assert_eq!(
        ViewerState::with_cache_config(CacheConfig::new(10, 2)).last_open_skipped(),
        0
    );
    assert_eq!(
        ViewerState::from_settings(&Settings::default()).last_open_skipped(),
        0
    );
}

#[test]
fn last_open_skipped_stays_zero_on_open_error() {
    // An open_path error must not update last_open_skipped; it stays 0.
    let mut state = ViewerState::new();
    let _ = state.open_path(std::path::Path::new("/nonexistent_path_pr6_skip"));
    assert_eq!(state.last_open_skipped(), 0);
}

// ---- open_path CBR/RAR dispatch (PR7) -----------------------------------

#[test]
fn open_path_nonexistent_cbr_returns_err_and_leaves_clean_state() {
    // A missing .cbr must error and leave a clean no-source state, proving .cbr
    // routes like .cbz/.zip. RarSource correctness lives in gashuu-core (no dev-dep).
    let mut state = ViewerState::new();
    let result = state.open_path(std::path::Path::new("/nonexistent_path_pr7_cbr_test.cbr"));
    assert!(
        result.is_err(),
        "open_path must return Err for a missing .cbr path"
    );
    assert_eq!(state.page_count(), 0, "page_count must stay 0 after error");
    assert_eq!(state.index(), 0, "index must stay 0 after error");
    assert!(
        state.decode_current_spread().is_none(),
        "decode_current_spread must be None after error"
    );
    assert_eq!(
        state.last_open_skipped(),
        0,
        "last_open_skipped must not update on error"
    );
}

// ---- current_source() (PR8a) ---------------------------------------------

#[test]
fn current_source_is_none_before_open() {
    // A freshly constructed ViewerState has no source installed yet.
    let state = ViewerState::new();
    assert!(
        state.current_source().is_none(),
        "current_source must be None before any open"
    );
}

#[test]
fn current_source_is_some_after_set_source() {
    // After set_source the Arc is retained and current_source returns Some.
    let mut state = ViewerState::new();
    state.set_source(mock_with(3));
    assert!(
        state.current_source().is_some(),
        "current_source must be Some after set_source"
    );
}

#[test]
fn current_source_is_none_after_failed_open_path() {
    // A failed open_path must NOT install a source; current_source stays None.
    let mut state = ViewerState::new();
    let _ = state.open_path(std::path::Path::new("/nonexistent_pr8a_source"));
    assert!(
        state.current_source().is_none(),
        "current_source must remain None after a failed open_path"
    );
}

// ---- open_file() (PR-R) --------------------------------------------------

#[test]
fn open_file_is_none_before_open() {
    let state = ViewerState::new();
    assert!(
        state.open_file().is_none(),
        "open_file must be None before any open"
    );
}

#[test]
fn open_file_is_none_after_failed_open_path() {
    let mut state = ViewerState::new();
    let _ = state.open_path(std::path::Path::new("/nonexistent_prR_open_file"));
    assert!(
        state.open_file().is_none(),
        "open_file must stay None after a failed open_path"
    );
}

#[test]
fn open_file_stays_none_after_direct_set_source() {
    // set_source has no path; open_file tracks the path given to open_path, so
    // after a direct set_source it stays None.
    let mut state = ViewerState::new();
    state.set_source(mock_with(3));
    assert!(
        state.open_file().is_none(),
        "set_source without a path must leave open_file as None"
    );
}

#[test]
fn open_file_is_some_canonical_after_successful_open_path() {
    // Linchpin of the write-back chain: a SUCCESSFUL open_path sets open_file to
    // Some(canonical). Exercised via a real on-disk directory (FolderSource).
    let dir = std::env::temp_dir().join(format!("gashuu_prR_openfile_{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("create temp dir");
    // An empty directory opens successfully as a FolderSource (confirmed by
    // gashuu-core's archive_loader tests), so no image file is needed here.

    let mut state = ViewerState::new();
    state
        .open_path(&dir)
        .expect("open_path on a real directory must succeed");

    let stored = state
        .open_file()
        .expect("open_file must be Some after a successful open_path");
    assert_eq!(
        stored,
        dir.canonicalize().expect("canonicalize temp dir"),
        "open_file must hold the canonical path"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

// ---- jump_to() (PR8a) ---------------------------------------------------

#[test]
fn jump_to_no_source_returns_false() {
    // With no source loaded jump_to must be a no-op.
    let mut state = ViewerState::new();
    assert!(
        !state.jump_to(0),
        "jump_to must return false with no source"
    );
    assert_eq!(state.index(), 0);
}

#[test]
fn jump_to_current_leading_returns_false() {
    // Jumping to the page already at the current leading is a no-op.
    let mut state = ViewerState::new();
    state.set_source(mock_with(5));
    // Default: Single mode, index 0 is the leading.
    assert!(
        !state.jump_to(0),
        "jump_to current leading must return false"
    );
    assert_eq!(state.index(), 0);
}

#[test]
fn jump_to_out_of_range_clamps() {
    // An out-of-range page is clamped to a valid leading without panic.
    let mut state = ViewerState::new();
    state.set_source(mock_with(4));
    // Single mode: every page is its own leading; page_count - 1 = 3.
    // Jumping to page_count + 5 = 9 must clamp to the last page (3).
    let moved = state.jump_to(9);
    assert!(moved, "jump_to out-of-range must move when index differs");
    assert_eq!(state.index(), 3, "clamped to last valid single leading");
}

#[test]
fn jump_to_single_mode_lands_on_exact_page() {
    // In Single mode every page is its own leading; jump_to should land there.
    let mut state = ViewerState::new();
    state.set_source(mock_with(6));
    assert!(state.jump_to(4));
    assert_eq!(state.index(), 4);
}

#[test]
fn jump_to_double_standalone_trailing_normalizes_to_leading() {
    // Double / Standalone: {0}{1,2}{3,4}{5}.
    // Clicking page 2 (trailing of the {1,2} spread) should land on leading 1.
    let mut state = double_state();
    state.set_source(mock_with(6));
    let moved = state.jump_to(2);
    assert!(moved, "jump_to trailing must move from cover (index 0)");
    assert_eq!(
        state.index(),
        1,
        "trailing page 2 must normalize to leading 1"
    );
}

#[test]
fn jump_to_double_standalone_trailing_page4_normalizes_to_leading3() {
    // Double / Standalone: {0}{1,2}{3,4}{5}.
    // Clicking page 4 (trailing of {3,4}) should land on leading 3.
    let mut state = double_state();
    state.set_source(mock_with(6));
    assert!(state.jump_to(4));
    assert_eq!(state.index(), 3);
}

#[test]
fn jump_to_double_paired_trailing_normalizes_to_leading() {
    // Double / Paired cover: {0,1}{2,3}{4,5}.
    // Clicking page 1 (trailing of {0,1}) should land on leading 0.
    let mut state = ViewerState::from_settings(&Settings {
        spread_mode: SpreadMode::Double,
        cover_mode: CoverMode::Paired,
        reading_direction: ReadingDirection::Ltr,
        ..Default::default()
    });
    state.set_source(mock_with(6));
    // index starts at 0; jump_to(1) should normalize to 0 => no move.
    assert!(
        !state.jump_to(1),
        "trailing page 1 normalizes to leading 0, no move from current 0"
    );
    assert_eq!(state.index(), 0);

    // Now jump_to the trailing page 3 of spread {2,3} -> leading 2.
    assert!(state.jump_to(3));
    assert_eq!(state.index(), 2);
}

#[test]
fn jump_to_double_paired_trailing_page5_normalizes_to_leading4() {
    // Double / Paired cover on 6 pages: {0,1}{2,3}{4,5}.
    // Clicking page 5 (trailing of {4,5}) should land on leading 4.
    let mut state = ViewerState::from_settings(&Settings {
        spread_mode: SpreadMode::Double,
        cover_mode: CoverMode::Paired,
        reading_direction: ReadingDirection::Ltr,
        ..Default::default()
    });
    state.set_source(mock_with(6));
    assert!(state.jump_to(5));
    assert_eq!(state.index(), 4);
}

// ---- jump_to resume behavior (PR-R) -----------------------------------

#[test]
fn jump_to_zero_is_noop_from_fresh_open() {
    // Library returns 0 for an unknown book; jump_to(0) on a freshly
    // set_source must NOT report a move (the viewer is already at index 0).
    let mut state = ViewerState::new();
    state.set_source(mock_with(5));
    assert_eq!(state.index(), 0);
    assert!(
        !state.jump_to(0),
        "jump_to(0) on a book just opened (index=0) must be a no-op"
    );
    assert_eq!(state.index(), 0);
}

#[test]
fn jump_to_stored_page_resumes_correctly() {
    // Simulates opening a book where resume_page = 3. Single mode: every page
    // is its own leading. jump_to(3) must move and land at index 3.
    let mut state = ViewerState::new();
    state.set_source(mock_with(10));
    let moved = state.jump_to(3);
    assert!(
        moved,
        "jump_to must return true when moving from index 0 to 3"
    );
    assert_eq!(state.index(), 3);
}

#[test]
fn jump_to_stored_trailing_normalizes_to_leading_on_resume() {
    // Double / Standalone: {0}{1,2}{3,4}{5}. If resume_page stored was 2
    // (trailing of {1,2}), jump_to(2) normalizes to leading 1.
    let mut state = double_state();
    state.set_source(mock_with(6));
    assert!(state.jump_to(2));
    assert_eq!(
        state.index(),
        1,
        "stored trailing page must normalize to spread leading on resume"
    );
}

// ---- open → read → leave sequence (PR-R borrow regression) -------------

#[test]
fn open_read_leave_sequence_state_invariants() {
    // Pins the borrow-regression invariants write_back_position relies on: open_file()
    // Some after open, index() tracks nav, and both read without a borrow conflict.

    let mut state = ViewerState::new();

    // set_source sets the cache but not open_file (open_path does that, tested
    // separately); here we assert only the fields we control in tests.
    state.set_source(mock_with(10));
    // After a direct set_source, open_file is None (no path); the happy path is
    // covered elsewhere. Here we verify index tracking and the read-shape.
    assert_eq!(state.index(), 0, "fresh after set_source: index is 0");

    // Read two pages (two spreads in Single mode).
    assert!(state.apply(NavAction::Next));
    assert!(state.apply(NavAction::Next));
    assert_eq!(state.index(), 2, "after two nexts: index is 2");

    // jump_to can be used for a scrubber seek too.
    assert!(state.jump_to(7));
    assert_eq!(state.index(), 7, "after jump_to(7): index is 7");

    // The reads write_back_position performs must not conflict in sequence
    // (distinct immutable borrows); this test pins the shape of those reads.
    let _page = state.index(); // what write_back_position calls
                               // open_file() is None here (set_source path), but the call must not panic.
    let _path = state.open_file(); // what write_back_position calls
                                   // No panic reached: the sequence is safe.

    // Simulate opening a second book (write_back fires for the first, then
    // set_source resets the state).
    state.set_source(mock_with(5));
    assert_eq!(state.index(), 0, "set_source resets index to 0");
    assert!(
        state.open_file().is_none(),
        "set_source without path leaves open_file None"
    );
}

// ---- scrub_fraction_to_page() (PR-S): pure fraction -> raw page ----------

#[test]
fn scrub_fraction_zero_count_is_zero_guard() {
    // No pages loaded: any fraction maps to page 0 and never divides by zero.
    assert_eq!(scrub_fraction_to_page(0.0, 0, false), 0);
    assert_eq!(scrub_fraction_to_page(0.5, 0, false), 0);
    assert_eq!(scrub_fraction_to_page(1.0, 0, true), 0);
}

#[test]
fn scrub_fraction_ltr_maps_ends_and_midpoint() {
    // 10 pages, LTR: f=0 -> page 0, f=1 -> page 9, f=0.5 -> middle. Span is the last
    // index (count-1) so both ends are reachable.
    assert_eq!(scrub_fraction_to_page(0.0, 10, false), 0);
    assert_eq!(scrub_fraction_to_page(1.0, 10, false), 9);
    // round(0.5 * 9) = round(4.5) = 5 (round-half-up via +0.5 floor).
    assert_eq!(scrub_fraction_to_page(0.5, 10, false), 5);
}

#[test]
fn scrub_fraction_rtl_inverts_fraction() {
    // RTL (manga): dragging LEFT advances, so the screen-left end (f=0) is the
    // LAST page and the screen-right end (f=1) is the FIRST page.
    assert_eq!(scrub_fraction_to_page(0.0, 10, true), 9);
    assert_eq!(scrub_fraction_to_page(1.0, 10, true), 0);
    // Midpoint is symmetric: round((1-0.5)*9) = round(4.5) = 5.
    assert_eq!(scrub_fraction_to_page(0.5, 10, true), 5);
}

#[test]
fn scrub_fraction_clamps_out_of_range_input() {
    // A knob dragged past either edge (Slint can report mouse_x outside the
    // track) clamps to [0,1] before mapping, so the page stays in range.
    assert_eq!(scrub_fraction_to_page(-0.4, 5, false), 0);
    assert_eq!(scrub_fraction_to_page(1.7, 5, false), 4);
    assert_eq!(scrub_fraction_to_page(-0.4, 5, true), 4); // RTL: under-left = last
    assert_eq!(scrub_fraction_to_page(1.7, 5, true), 0); // RTL: over-right = first
}

#[test]
fn scrub_fraction_rounds_half_up_at_exact_half_step() {
    // Pin round-half-up at an EXACT half-page boundary (not the midpoint), where a
    // round-half-down/to-even impl would diverge. 5 pages => span 4; 0.125*4+0.5=1.0.
    assert_eq!(scrub_fraction_to_page(0.125, 5, false), 1);
    //   The boundary between page 1 and 2 sits at 1.5/4 = 0.375.
    //   0.375 * 4 + 0.5 = 2.0 -> page 2.
    assert_eq!(scrub_fraction_to_page(0.375, 5, false), 2);
    //   Just below a half-step rounds DOWN: 0.124 * 4 + 0.5 = 0.996 -> page 0.
    assert_eq!(scrub_fraction_to_page(0.124, 5, false), 0);

    // RTL inverts the fraction BEFORE rounding, so +0.5 half-up applies to (1-frac):
    // frac 0.875 => (1-0.875)*4+0.5 = 1.0 -> page 1 (page-0/1 boundary from the right).
    assert_eq!(scrub_fraction_to_page(0.875, 5, true), 1);
    //   frac = 1 - 0.375 = 0.625 -> (1 - 0.625) * 4 + 0.5 = 2.0 -> page 2.
    assert_eq!(scrub_fraction_to_page(0.625, 5, true), 2);
}

#[test]
fn scrub_fraction_non_finite_single_page_is_zero() {
    // Two zero-guards stack: a single page has span 0 AND a non-finite fraction is
    // coerced to 0.0. Either alone forces page 0; together must still be 0, never panic.
    assert_eq!(scrub_fraction_to_page(f32::NAN, 1, true), 0);
    assert_eq!(scrub_fraction_to_page(f32::INFINITY, 1, false), 0);
}

#[test]
fn scrub_fraction_odd_span_rtl_half_step_rounds_half_up() {
    // Odd span (4 pages, index 3) at an exact RTL half-step: (1-0.5)*3=1.5 rounds UP
    // to 2 (half-down gives 1). 0.5 and 1.5 are f32-exact so this is not flaky.
    assert_eq!(scrub_fraction_to_page(0.5, 4, true), 2);
}

#[test]
fn scrub_fraction_single_page_is_always_zero() {
    // A 1-page book: the only valid index is 0 regardless of fraction/dir
    // (count-1 == 0 span; f * 0 == 0).
    assert_eq!(scrub_fraction_to_page(0.0, 1, false), 0);
    assert_eq!(scrub_fraction_to_page(1.0, 1, false), 0);
    assert_eq!(scrub_fraction_to_page(0.3, 1, true), 0);
}

#[test]
fn scrub_fraction_is_total_function_no_nan_panic() {
    // A non-finite fraction (degenerate Slint length ratio) must not panic and is
    // coerced to f=0.0: NaN/LTR -> page 0; +Inf/RTL inverts to 1.0 -> page 7.
    let p = scrub_fraction_to_page(f32::NAN, 8, false);
    assert_eq!(p, 0);
    let p2 = scrub_fraction_to_page(f32::INFINITY, 8, true);
    assert_eq!(p2, 7);
}

#[test]
fn preview_spread_normalizes_double_paired_raw_page() {
    let mut state = ViewerState::from_settings(&Settings {
        spread_mode: SpreadMode::Double,
        cover_mode: CoverMode::Paired,
        ..Default::default()
    });
    state.set_source(mock_with(10));

    assert_eq!(
        state.preview_spread(3),
        Some(spread_at(10, SpreadLayout::Double, CoverMode::Paired, 2))
    );
}

#[test]
fn preview_spread_double_paired_tail_uses_distinct_pages() {
    let mut state = ViewerState::from_settings(&Settings {
        spread_mode: SpreadMode::Double,
        cover_mode: CoverMode::Paired,
        ..Default::default()
    });
    state.set_source(mock_with(2));

    assert_eq!(
        state.preview_spread(1),
        Some(spread_at(2, SpreadLayout::Double, CoverMode::Paired, 0))
    );
}

#[test]
fn preview_spread_double_standalone_cover_is_single() {
    let mut state = double_state();
    state.set_source(mock_with(10));

    assert_eq!(
        state.preview_spread(0),
        Some(spread_at(
            10,
            SpreadLayout::Double,
            CoverMode::Standalone,
            0
        ))
    );
}

#[test]
fn preview_spread_single_mode_keeps_requested_page() {
    let mut state = ViewerState::new();
    state.set_source(mock_with(10));
    let page = 3;

    assert_eq!(
        state.preview_spread(page),
        Some(spread_at(
            10,
            SpreadLayout::Single,
            CoverMode::Standalone,
            page
        ))
    );
}

#[test]
fn preview_spread_is_none_with_no_source() {
    let state = ViewerState::new();

    assert_eq!(state.preview_spread(3), None);
}

#[test]
fn preview_is_double_matches_spread_layout() {
    // Double / Standalone, 6 pages: {0}{1,2}{3,4}{5}. Cover (0) and last odd (5) are
    // single, inner pairs double — preview_is_double must report this WITHOUT moving.
    let mut state = double_state();
    state.set_source(mock_with(6));
    assert!(!state.preview_is_double(0)); // cover stands alone
    assert!(state.preview_is_double(1)); // {1,2}
    assert!(state.preview_is_double(2)); // page 2 normalizes to leading 1 -> double
    assert!(state.preview_is_double(3)); // {3,4}
    assert!(!state.preview_is_double(5)); // last odd stands alone
    assert_eq!(state.index(), 0, "preview must not move the index");
}

#[test]
fn preview_is_double_false_with_no_source() {
    let state = ViewerState::new();
    assert!(!state.preview_is_double(0));
}

#[test]
fn preview_is_double_single_mode_always_false() {
    let mut state = ViewerState::new(); // Single by default
    state.set_source(mock_with(6));
    assert!(!state.preview_is_double(0));
    assert!(!state.preview_is_double(3));
}

#[test]
fn preview_is_double_paired_cover_is_double() {
    // Double + Paired, 6 pages: spreads {0,1}{2,3}{4,5}. Unlike Standalone,
    // the cover (page 0) pairs with page 1, so it is a DOUBLE spread.
    let mut state = ViewerState::from_settings(&Settings {
        spread_mode: SpreadMode::Double,
        cover_mode: CoverMode::Paired,
        ..Default::default()
    });
    state.set_source(mock_with(6));
    assert!(state.preview_is_double(0)); // cover {0,1} -> double in Paired
    assert!(state.preview_is_double(1)); // page 1 normalizes to leading 0 -> {0,1}
    assert!(state.preview_is_double(4)); // {4,5}
    assert_eq!(state.index(), 0, "preview must not move the index");

    // 5 pages, Double + Paired: {0,1}{2,3}{4}. The lone last page is single.
    let mut state = ViewerState::from_settings(&Settings {
        spread_mode: SpreadMode::Double,
        cover_mode: CoverMode::Paired,
        ..Default::default()
    });
    state.set_source(mock_with(5));
    assert!(state.preview_is_double(0)); // {0,1}
    assert!(!state.preview_is_double(4)); // {4} lone last -> single
}

#[test]
fn scrub_commit_path_jumps_via_jump_to() {
    // The commit seam (on_scrub_commit) resolves the RAW Slint release fraction via
    // scrub_fraction_to_page (single source of rounding incl. RTL), then jump_to.
    let mut state = ViewerState::new();
    state.set_source(mock_with(8));
    let page = scrub_fraction_to_page(1.0, state.page_count(), false);
    assert_eq!(page, 7);
    assert!(state.jump_to(page));
    assert_eq!(state.index(), 7);

    // RTL: screen-left release (fraction 0.0) -> last page in reading order.
    let mut state = ViewerState::from_settings(&Settings {
        reading_direction: ReadingDirection::Rtl,
        ..Default::default()
    });
    state.set_source(mock_with(8));
    let rtl = matches!(state.reading_direction(), ReadingDirection::Rtl);
    let page = scrub_fraction_to_page(0.0, state.page_count(), rtl);
    assert_eq!(page, 7);
    assert!(state.jump_to(page));
    assert_eq!(state.index(), 7);
}

// ---- set_spread_mode (PR8b) ---------------------------------------------

#[test]
fn set_spread_mode_to_double_renormalizes_index() {
    // Single at index 2 of 6. Switching to Double/Standalone makes index 2 (even>0)
    // an invalid leading, so renormalize re-anchors to pair start 1; returns true.
    let mut state = ViewerState::new();
    state.set_source(mock_with(6));
    state.apply(NavAction::Next);
    state.apply(NavAction::Next);
    assert_eq!(state.index(), 2);
    assert_eq!(state.spread_mode(), SpreadMode::Single);

    assert!(state.set_spread_mode(SpreadMode::Double));
    assert_eq!(state.spread_mode(), SpreadMode::Double);
    // index 2 normalized to valid Standalone Double leading 1.
    assert_eq!(state.index(), 1);
}

#[test]
fn set_spread_mode_same_value_is_noop() {
    // Calling set_spread_mode with the already-active mode must return false
    // and leave index unchanged.
    let mut state = ViewerState::new();
    state.set_source(mock_with(6));
    state.apply(NavAction::Next);
    state.apply(NavAction::Next);
    assert_eq!(state.index(), 2);
    assert_eq!(state.spread_mode(), SpreadMode::Single);

    assert!(!state.set_spread_mode(SpreadMode::Single));
    assert_eq!(state.spread_mode(), SpreadMode::Single);
    assert_eq!(state.index(), 2);
}

// ---- set_cover_mode (PR8b) ----------------------------------------------

#[test]
fn set_cover_mode_flips_and_renormalizes() {
    // Double / Standalone at index 5 of 6. Switching to Paired makes pairs
    // start even, so index 5 normalizes down to the even pair start 4 ({4,5}).
    let mut state = double_state();
    state.set_source(mock_with(6));
    for _ in 0..3 {
        state.apply(NavAction::Next);
    }
    assert_eq!(state.index(), 5);
    assert_eq!(state.cover_mode(), CoverMode::Standalone);

    assert!(state.set_cover_mode(CoverMode::Paired));
    assert_eq!(state.cover_mode(), CoverMode::Paired);
    assert_eq!(state.index(), 4);

    // Setting it back to Standalone: page 4 (even>0) normalizes to pair
    // start 3 ({3,4}) in Standalone Double.
    assert!(state.set_cover_mode(CoverMode::Standalone));
    assert_eq!(state.cover_mode(), CoverMode::Standalone);
    assert_eq!(state.index(), 3);
}

#[test]
fn set_cover_mode_same_value_is_noop() {
    // Calling set_cover_mode with the already-active mode must return false
    // and leave index unchanged.
    let mut state = double_state();
    state.set_source(mock_with(6));
    for _ in 0..3 {
        state.apply(NavAction::Next);
    }
    assert_eq!(state.index(), 5);
    assert_eq!(state.cover_mode(), CoverMode::Standalone);

    assert!(!state.set_cover_mode(CoverMode::Standalone));
    assert_eq!(state.cover_mode(), CoverMode::Standalone);
    assert_eq!(state.index(), 5);
}

// ---- set_reading_direction (PR8b) ----------------------------------------

#[test]
fn set_reading_direction_flips_and_leaves_index() {
    // Double / Standalone, Ltr. Switching to Rtl returns true; pairing is
    // direction-agnostic so index must remain unchanged.
    let mut state = double_state();
    state.set_source(mock_with(6));
    state.apply(NavAction::Next);
    assert_eq!(state.index(), 1);
    assert_eq!(state.reading_direction(), ReadingDirection::Ltr);

    assert!(state.set_reading_direction(ReadingDirection::Rtl));
    assert_eq!(state.reading_direction(), ReadingDirection::Rtl);
    assert_eq!(state.index(), 1);

    assert!(state.set_reading_direction(ReadingDirection::Ltr));
    assert_eq!(state.reading_direction(), ReadingDirection::Ltr);
    assert_eq!(state.index(), 1);
}

#[test]
fn set_reading_direction_same_value_is_noop() {
    // Calling set_reading_direction with the already-active direction must
    // return false and leave index unchanged.
    let mut state = double_state();
    state.set_source(mock_with(6));
    state.apply(NavAction::Next);
    assert_eq!(state.index(), 1);
    assert_eq!(state.reading_direction(), ReadingDirection::Ltr);

    assert!(!state.set_reading_direction(ReadingDirection::Ltr));
    assert_eq!(state.reading_direction(), ReadingDirection::Ltr);
    assert_eq!(state.index(), 1);
}

// ---- set_cache_config (PR8b) ---------------------------------------------

#[test]
fn set_spread_mode_to_auto_portrait_renormalizes_like_double() {
    // Single at index 2 of 6, portrait viewport => Auto resolves to Double. index 2
    // (even>0) isn't a valid Standalone Double leading, so it re-anchors to 1; true.
    let mut state = ViewerState::new();
    state.set_source(mock_with(6));
    state.apply(NavAction::Next);
    state.apply(NavAction::Next);
    assert_eq!(state.index(), 2);
    assert_eq!(state.spread_mode(), SpreadMode::Single);

    // Portrait viewport: Auto resolves to Double.
    state.set_viewport_size(900.0, 1200.0);

    assert!(state.set_spread_mode(SpreadMode::Auto));
    assert_eq!(state.spread_mode(), SpreadMode::Auto);
    // Index 2 normalized to valid Standalone Double leading 1 ({1,2}).
    assert_eq!(state.index(), 1);
}

#[test]
fn set_spread_mode_to_auto_landscape_preserves_index() {
    // Single at index 2 of 6, landscape viewport => Auto resolves to Single (every
    // page its own leading). Switch returns true, but index 2 stays a valid leading.
    let mut state = ViewerState::new();
    state.set_source(mock_with(6));
    state.apply(NavAction::Next);
    state.apply(NavAction::Next);
    assert_eq!(state.index(), 2);
    assert_eq!(state.spread_mode(), SpreadMode::Single);

    // Landscape viewport: Auto resolves to Single.
    state.set_viewport_size(1600.0, 900.0);

    assert!(state.set_spread_mode(SpreadMode::Auto));
    assert_eq!(state.spread_mode(), SpreadMode::Auto);
    // Index 2 is a valid Single leading; renormalize is idempotent here.
    assert_eq!(state.index(), 2);
}

#[test]
fn set_cover_mode_preserves_valid_leading() {
    // Double/Standalone at index 0 (cover). Switching to Paired ({0,1}{2,3}{4,5}), 0
    // is already valid: returns true but index stays 0 (renormalize idempotent).
    let mut state = double_state();
    state.set_source(mock_with(6));
    assert_eq!(state.index(), 0);
    assert_eq!(state.cover_mode(), CoverMode::Standalone);

    assert!(state.set_cover_mode(CoverMode::Paired));
    assert_eq!(state.cover_mode(), CoverMode::Paired);
    // Index 0 is a valid Paired leading; renormalize must leave it at 0.
    assert_eq!(state.index(), 0);
}

#[test]
fn set_cache_config_updates_fields() {
    // set_cache_config updates the fields set_source reads on the next open, so a
    // settings dialog's new cache/preload values apply to the next book, no relaunch.
    let mut state = ViewerState::new();
    // radius 7 exceeds MAX_PREFETCH_RADIUS (5) and is clamped by CacheConfig::new.
    state.set_cache_config(CacheConfig::new(99, 7));
    assert_eq!(state.cache_config().capacity(), 99);
    assert_eq!(state.cache_config().radius(), 5);
}

#[test]
fn apply_resolved_view_sets_direction_spread_cover() {
    // Defaults are Single/Standalone/Ltr; the resolved view differs on all three so
    // asserts aren't vacuous.
    let mut s = ViewerState::new();
    let mut viewport = ViewportState::from_settings(&Settings::default());
    s.apply_resolved_view(
        ResolvedView {
            reading_direction: ReadingDirection::Rtl,
            spread_mode: SpreadMode::Double,
            cover_mode: CoverMode::Paired,
            fit_mode: gashuu_core::FitMode::Actual,
        },
        &mut viewport,
    );
    assert_eq!(s.reading_direction(), ReadingDirection::Rtl);
    assert_eq!(s.spread_mode(), SpreadMode::Double);
    assert_eq!(s.cover_mode(), CoverMode::Paired);
}

#[test]
fn apply_resolved_view_sets_fit_and_resets_zoom() {
    let mut state = ViewerState::new();
    let mut viewport = ViewportState::from_settings(&Settings::default());
    viewport.resize(200.0, 200.0);
    viewport.set_content(200.0, 200.0);
    viewport.zoom_step(true);
    assert!(
        viewport.geometry().2 > 200.0 * gashuu_core::ZOOM_MIN,
        "test setup must start zoomed above ZOOM_MIN"
    );

    state.apply_resolved_view(
        ResolvedView {
            reading_direction: ReadingDirection::Ltr,
            spread_mode: SpreadMode::Single,
            cover_mode: CoverMode::Standalone,
            fit_mode: gashuu_core::FitMode::Actual,
        },
        &mut viewport,
    );

    assert_eq!(viewport.fit_mode(), gashuu_core::FitMode::Actual);
    assert_eq!(
        viewport.geometry().2,
        200.0 * gashuu_core::ZOOM_MIN,
        "applying the resolved fit must reset zoom to ZOOM_MIN"
    );
}

#[test]
fn close_returns_to_no_book_open_state() {
    // close() drops the source and reports the boot/no-folder shape (no source, zero
    // pages/index, open_file None, status NoFolder). Used by bulk-removal of the open book.
    let mut state = ViewerState::new();
    state.set_source(mock_with(5));
    assert_eq!(state.page_count(), 5);
    assert!(state.current_source().is_some());

    state.close();
    assert_eq!(state.page_count(), 0, "page count zeroed on close");
    assert_eq!(state.index(), 0, "index reset on close");
    assert!(state.current_source().is_none(), "source dropped on close");
    assert!(state.open_file().is_none(), "open_file cleared on close");
    assert!(
        state.decode_current_spread().is_none(),
        "no spread after close"
    );
    assert_eq!(
        state.status_content().kind,
        StatusKind::NoFolder,
        "status reverts to NoFolder after close"
    );
}

#[test]
fn close_preserves_display_modes() {
    // Closing a book is not a settings reset: the runtime display modes survive
    // so the NEXT open reuses them (the apply_resolved_view path then overrides).
    let mut state = double_state();
    state.set_source(mock_with(4));
    assert_eq!(state.spread_mode(), SpreadMode::Double);

    state.close();
    assert_eq!(
        state.spread_mode(),
        SpreadMode::Double,
        "close must not reset the spread mode"
    );
}

#[test]
fn close_is_idempotent_from_boot_state() {
    // Closing when nothing is open must be a harmless no-op (no panic, stays empty).
    let mut state = ViewerState::new();
    state.close();
    assert_eq!(state.page_count(), 0);
    assert!(state.open_file().is_none());
    assert_eq!(state.status_content().kind, StatusKind::NoFolder);
}

// ---- page_count_opt() (DDD Wave 1) --------------------------------------

#[test]
fn page_count_opt_is_none_when_empty() {
    // No source open: the 0 sentinel maps to None (mirrors Book::page_count_opt).
    let state = ViewerState::new();
    assert_eq!(state.page_count(), 0);
    assert!(state.page_count_opt().is_none());

    // An open source with zero displayable pages is also "empty" -> None.
    let mut state = ViewerState::new();
    state.set_source(mock_with(0));
    assert_eq!(state.page_count(), 0);
    assert!(state.page_count_opt().is_none());
}

#[test]
fn page_count_opt_is_some_with_real_count() {
    // A positive page count surfaces as Some(NonZeroUsize) with the exact value,
    // while the raw page_count() accessor stays in lockstep.
    let mut state = ViewerState::new();
    state.set_source(mock_with(5));
    assert_eq!(state.page_count(), 5);
    assert_eq!(state.page_count_opt().map(NonZeroUsize::get), Some(5));
}

// ---- inherit-pending guard (#415: reset-to-global undone on close) ------

#[test]
fn inherit_pending_defaults_false() {
    // A freshly constructed state pins normally (no reset performed).
    assert!(!ViewerState::new().is_inherit_pending());
}

#[test]
fn mark_inherit_pending_sets_and_setters_clear_it() {
    // A "Reset to global" marks the book inherit-pending...
    let mut state = double_state();
    state.mark_inherit_pending();
    assert!(state.is_inherit_pending());

    // ...and a subsequent REAL reading-direction change clears it, so the next
    // write-back pins the runtime again (the guard must not block re-selection).
    assert!(state.set_reading_direction(ReadingDirection::Rtl));
    assert!(!state.is_inherit_pending());
}

#[test]
fn mark_inherit_pending_cleared_by_spread_and_cover_setters() {
    // Each mode setter that actually changes a value re-enables pinning.
    let mut state = double_state();
    state.mark_inherit_pending();
    assert!(state.set_spread_mode(SpreadMode::Single));
    assert!(!state.is_inherit_pending());

    state.mark_inherit_pending();
    assert!(state.set_cover_mode(CoverMode::Paired));
    assert!(!state.is_inherit_pending());
}

#[test]
fn mark_inherit_pending_cleared_by_keyboard_toggles() {
    // Keyboard D/R/C toggles are real mode changes too, so they clear the flag.
    let mut state = double_state();
    state.mark_inherit_pending();
    assert!(state.toggle_spread());
    assert!(!state.is_inherit_pending());

    state.mark_inherit_pending();
    assert!(state.toggle_cover());
    assert!(!state.is_inherit_pending());

    state.mark_inherit_pending();
    assert!(state.toggle_reading_direction());
    assert!(!state.is_inherit_pending());
}

#[test]
fn idempotent_setter_keeps_inherit_pending() {
    // Selecting the SAME value is a no-op change: the book keeps inheriting, so
    // the flag must survive (only a real deviation re-pins).
    let mut state = double_state(); // Rtl-default settings? built with Ltr direction.
    state.mark_inherit_pending();
    assert!(!state.set_reading_direction(ReadingDirection::Ltr)); // already Ltr
    assert!(state.is_inherit_pending());
}

#[test]
fn set_source_clears_inherit_pending() {
    // Opening (or replacing) a book drops the previous book's inherit intent.
    let mut state = double_state();
    state.mark_inherit_pending();
    state.set_source(mock_with(3));
    assert!(!state.is_inherit_pending());
}
