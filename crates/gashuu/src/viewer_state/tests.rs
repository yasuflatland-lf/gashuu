use super::*;
use gashuu_core::{MockPageSource, PageEntry};
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

// ---- Delegation seam ---------------------------------------------------

#[test]
fn preview_is_double_delegates_to_the_navigation_value_object() {
    // The pairing table itself lives in `gashuu_core::spread`
    // (`navigation_preview_is_double_cases`); this test pins only the
    // `ViewerState` -> `SpreadNavigation` seam, which no other UI test crosses:
    // the scrubber preview (`main.rs`) reads `preview_is_double` and nothing
    // else would go red if the delegation were replaced by a constant.
    //
    // 6 pages, Double / Standalone: {0}{1,2}{3,4}{5}. Both a `true` and a
    // `false` row are asserted so neither constant survives.
    let mut state = double_state();
    state.set_source(mock_with(6));
    assert!(!state.preview_is_double(0), "cover stands alone");
    assert!(state.preview_is_double(1), "{{1,2}} is a double spread");
    assert!(!state.preview_is_double(5), "last odd page stands alone");
    assert_eq!(state.index(), 0, "preview must not move the index");

    // No source: the seam must report `false` rather than panic.
    assert!(!ViewerState::new().preview_is_double(0));
}

// ---- Toggles -----------------------------------------------------------

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
// ---- toggle_spread from Double/Paired preserves the visible page (FIX 7)
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
// ---- jump_to resume behavior (PR-R) -----------------------------------
// ---- open → read → leave sequence (PR-R borrow regression) -------------

#[test]
fn open_read_leave_sequence_state_invariants() {
    // Pins the borrow-regression invariants stage_position_write_back relies on: open_file()
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

    // The reads stage_position_write_back performs must not conflict in sequence
    // (distinct immutable borrows); this test pins the shape of those reads.
    let _page = state.resume_index_to_persist(); // what write-back calls
                                                 // open_file() is None here (set_source path), but the call must not panic.
    let _path = state.open_file(); // what stage_position_write_back calls
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
