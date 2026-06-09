use crate::carousel::{thumb_state_at, ThumbState};
use crate::keymap::{map_key, KeyCommand};
use crate::library_model::{LibrarySearchState, LibrarySelectionState};
use crate::navigation::NavState;
use crate::page_jump::parse_page_jump;
use crate::page_loader::PageController;
use crate::viewer_state::{scrub_fraction_to_page, ViewerState};
use crate::viewport::ViewportState;
use crate::{
    apply_spread_geometry, apply_viewport, clear_page_view, current_page_1based, go_to_library,
    persist_view_modes, refresh, with_ui, write_back_position, CarouselRefresh, ViewModeRoute,
    ViewerWindow,
};
use crate::{cover_loader, i18n};
use gashuu_core::{FitMode, Library, ReadingDirection, Settings};
use slint::ComponentHandle;
use std::cell::RefCell;
use std::rc::Rc;

/// Registers the viewer input callbacks (thumbnails, page jump, chrome reveal, scrubber, thumbnail-strip toggle) onto `ui`.
/// Panel constraint (#151): explicit handle list IS the dependency list — no AppState bundle.
pub(crate) fn wire_viewer_input_handlers(
    ui: &ViewerWindow,
    state: &Rc<RefCell<ViewerState>>,
    viewport: &Rc<RefCell<ViewportState>>,
    pages: &Rc<PageController>,
    localizer: &Rc<i18n::Localizer>,
) {
    let state = Rc::clone(state);
    let viewport = Rc::clone(viewport);
    let pages = Rc::clone(pages);
    let localizer = Rc::clone(localizer);

    // Async page-decode bridge: `page_loader` applies Slint images on the event
    // loop, then invokes this scalar callback so the UI-thread-only viewport and
    // localizer can update geometry/status.
    {
        let ui_weak = ui.as_weak();
        let state = Rc::clone(&state);
        let viewport = Rc::clone(&viewport);
        let pages = Rc::clone(&pages);
        let localizer = Rc::clone(&localizer);
        ui.on_spread_anchored(
            move |content_w, content_h, single, trailing_failed, leading_idx, trailing_idx| {
                with_ui(&ui_weak, |ui| {
                    let leading = leading_idx.max(0) as usize;
                    let trailing = (trailing_idx >= 0).then_some(trailing_idx as usize);
                    pages.clear_dispatched_spread(leading, trailing);
                    ui.set_leading_loading(false);
                    ui.set_trailing_loading(false);
                    // Unmounting the LoadingSlot perturbs PageView's TouchArea
                    // hit-testing the same way the mount does (see main.rs MISS
                    // path); suppress the phantom pointer-reveal so the decode
                    // finish does not flip the chrome under a stationary cursor.
                    ui.invoke_arm_pointer_reveal_suppression();
                    let failed = (trailing_failed >= 0).then_some(trailing_failed as usize);
                    let status = state.borrow().status_content();
                    apply_spread_geometry(
                        &ui,
                        &viewport,
                        localizer.loader(),
                        content_w,
                        content_h,
                        single,
                        failed,
                        &status,
                    );
                })
            },
        );
    }

    // Decode failures are surfaced only through this async callback. The worker
    // logged the concrete core error before sending the scalar page index.
    {
        let ui_weak = ui.as_weak();
        let viewport = Rc::clone(&viewport);
        let pages = Rc::clone(&pages);
        let localizer = Rc::clone(&localizer);
        ui.on_page_decode_error(move |page_index| {
            with_ui(&ui_weak, |ui| {
                let page = page_index.max(0) as usize;
                pages.clear_dispatched(page);
                let detail = format!("page {}", page + 1);
                ui.set_status_text(
                    crate::i18n::dynamic::decode_error(localizer.loader(), &detail).into(),
                );
                clear_page_view(&ui, &viewport);
                // clear_page_view unmounts the LoadingSlot (loading flags →
                // false), which perturbs PageView's TouchArea hit-testing under a
                // stationary cursor; suppress the phantom pointer-reveal so a
                // decode error does not flip the chrome (mirrors on_spread_anchored).
                ui.invoke_arm_pointer_reveal_suppression();
            })
        });
    }

    // Thumbnail click: jump to the clicked page's spread, refresh, then restore
    // focus to the page area so keyboard navigation keeps working.
    {
        let ui_weak = ui.as_weak();
        let state = Rc::clone(&state);
        let viewport = Rc::clone(&viewport);
        let pages = Rc::clone(&pages);
        let localizer = Rc::clone(&localizer);
        ui.on_thumbnail_clicked(move |page| {
            with_ui(&ui_weak, |ui| {
                if state.borrow_mut().jump_to(page as usize) {
                    refresh(
                        &ui,
                        &state.borrow(),
                        &viewport,
                        localizer.loader(),
                        &pages,
                        ui.as_weak(),
                    );
                }
                ui.invoke_focus_pages();
            })
        });
    }

    // On invalid or no-op input the field snaps back to the current page; this
    // is the feedback mechanism instead of a visible error message.
    {
        let ui_weak = ui.as_weak();
        let state = Rc::clone(&state);
        let viewport = Rc::clone(&viewport);
        let pages = Rc::clone(&pages);
        let localizer = Rc::clone(&localizer);
        ui.on_page_jump_request(move |text: slint::SharedString| {
            with_ui(&ui_weak, |ui| {
                let total = state.borrow().page_count();
                let did_jump = if let Some(page_0based) = parse_page_jump(text.as_str(), total) {
                    let moved = state.borrow_mut().jump_to(page_0based);
                    if moved {
                        refresh(
                            &ui,
                            &state.borrow(),
                            &viewport,
                            localizer.loader(),
                            &pages,
                            ui.as_weak(),
                        );
                        // Belt-and-suspenders: if refresh gains an early-return path,
                        // the field still shows the canonical post-jump page.
                        ui.set_page_jump_text(
                            format!("{}", current_page_1based(&state.borrow())).into(),
                        );
                    }
                    moved
                } else {
                    tracing::debug!(input = %text, "page_jump: invalid input, restoring");
                    false
                };
                if !did_jump {
                    ui.set_page_jump_text(
                        format!("{}", current_page_1based(&state.borrow())).into(),
                    );
                }
                ui.invoke_focus_pages();
            })
        });
    }

    // Page-jump cancel: restore the field to the current page.
    {
        let ui_weak = ui.as_weak();
        let state = Rc::clone(&state);
        ui.on_page_jump_cancel(move || {
            with_ui(&ui_weak, |ui| {
                ui.set_page_jump_text(format!("{}", current_page_1based(&state.borrow())).into());
                ui.invoke_focus_pages();
            })
        });
    }

    // Reveal the auto-hiding chrome and re-arm its idle-fade countdown. Fired on
    // mouse-move over the page and on a scrubber drag (arrow turns reveal via the
    // `nav` handler below).
    {
        let ui_weak = ui.as_weak();
        ui.on_reveal_chrome(move || {
            with_ui(&ui_weak, |ui| {
                ui.invoke_reveal_chrome_now();
            })
        });
    }

    // Drag preview: update ONLY the popover thumbnails + counter for the page
    // under the knob. The page body is unchanged until commit (spec decision 11).
    // Thumbnails are pulled from the EXISTING PR8a VecModel<ThumbnailItem> by page
    // index — no new decode, UI thread only (the Rc model is never crossed).
    {
        let ui_weak = ui.as_weak();
        let state = Rc::clone(&state);
        ui.on_scrub_preview(move |frac| {
            with_ui(&ui_weak, |ui| {
                let total = state.borrow().page_count();
                if total == 0 {
                    return;
                }
                // Single source of rounding: resolve the raw knob fraction to a
                // 0-based page via the pure `scrub_fraction_to_page` (clamp, RTL
                // inversion, round-half-up) — the Slint side no longer rounds.
                let rtl = matches!(state.borrow().reading_direction(), ReadingDirection::Rtl);
                let lead = scrub_fraction_to_page(frac, total, rtl);
                // Decide whether this previewed spread is double using the SAME layout
                // resolution the body uses, so the popover shows 1 vs 2 thumbs
                // correctly (the pure helper carries no layout; ViewerState owns it).
                let is_double = state.borrow().preview_is_double(lead);
                ui.set_scrubber_double(is_double);
                // Trailing page of the previewed spread (clamped to the last page),
                // present only for a double spread.
                let trail = if is_double {
                    Some((lead + 1).min(total - 1))
                } else {
                    None
                };
                // Pull thumbnail state (image + loaded/failed flags) from the
                // existing model (no decode) and push it so the popover renders
                // the loading/failed placeholder, not a blank cell.
                let model = ui.get_thumbnails();
                let a = thumb_state_at(&model, lead);
                ui.set_scrubber_preview_a(a.image);
                ui.set_scrubber_preview_a_loaded(a.loaded);
                ui.set_scrubber_preview_a_failed(a.failed);
                let b = match trail {
                    Some(trail) => thumb_state_at(&model, trail),
                    None => ThumbState::loading(),
                };
                ui.set_scrubber_preview_b(b.image);
                ui.set_scrubber_preview_b_loaded(b.loaded);
                ui.set_scrubber_preview_b_failed(b.failed);
                // Keep the chrome visible during the drag.
                ui.invoke_reveal_chrome_now();
            })
        });
    }

    // Commit on release: jump to the spread containing the released page, then
    // refresh (which re-seeds the scrubber + counter to the committed spread).
    {
        let ui_weak = ui.as_weak();
        let state = Rc::clone(&state);
        let viewport = Rc::clone(&viewport);
        let pages = Rc::clone(&pages);
        let localizer = Rc::clone(&localizer);
        ui.on_scrub_commit(move |frac| {
            with_ui(&ui_weak, |ui| {
                // Single source of rounding: resolve the raw release fraction to a
                // page via `scrub_fraction_to_page` (the same helper the preview
                // path uses), then jump. `page_count`/`reading_direction` reads and
                // the `borrow_mut()` jump_to each drop at their `;` before refresh
                // takes a fresh borrow.
                let total = state.borrow().page_count();
                let rtl = matches!(state.borrow().reading_direction(), ReadingDirection::Rtl);
                let page = scrub_fraction_to_page(frac, total, rtl);
                // Refresh unconditionally — unlike the nav handler, a scrub commit always
                // re-seeds the scrubber knob + counter to the committed spread, even when
                // the resolved leading equals the current index (a no-op jump).
                let _moved = state.borrow_mut().jump_to(page);
                refresh(
                    &ui,
                    &state.borrow(),
                    &viewport,
                    localizer.loader(),
                    &pages,
                    ui.as_weak(),
                );
                ui.invoke_focus_pages();
            })
        });
    }

    // Toggle the thumbnail strip's visibility. No refresh needed: showing/hiding
    // the strip changes PageView's height, which fires the existing
    // `viewport-resized` wiring automatically.
    {
        let ui_weak = ui.as_weak();
        ui.on_toggle_thumbnails(move || {
            with_ui(&ui_weak, |ui| {
                ui.set_show_thumbnails(!ui.get_show_thumbnails());
            })
        });
    }
}

/// Registers the viewport geometry callbacks (resize, zoom, pan) onto `ui`.
/// Panel constraint (#151): explicit handle list IS the dependency list — no AppState bundle.
pub(crate) fn wire_viewport_handlers(ui: &ViewerWindow, viewport: &Rc<RefCell<ViewportState>>) {
    let viewport = Rc::clone(viewport);

    // Zoom/pan input callbacks forwarded from PageView via ViewerWindow.
    // Each updates the `ViewportState` and re-pushes geometry.
    // Borrow-scoping rule: never hold a `borrow_mut()` while constructing the
    // `&viewport.borrow()` argument to `apply_viewport`. Mutate in one statement
    // (the temporary `borrow_mut` drops at the `;`), then take a fresh immutable
    // borrow for apply.
    {
        let ui_weak = ui.as_weak();
        let viewport = Rc::clone(&viewport);
        ui.on_viewport_resized(move |w, h| {
            with_ui(&ui_weak, |ui| {
                viewport.borrow_mut().resize(w, h);
                apply_viewport(&ui, &viewport.borrow());
            })
        });
    }
    {
        let ui_weak = ui.as_weak();
        let viewport = Rc::clone(&viewport);
        // `dy` is Slint's wheel `delta-y / 1px` passed straight through; the
        // zoom-in/out sign convention lives in `ViewportState::zoom_at`
        // (raw_delta > 0 = zoom in). If manual testing shows the wheel feels
        // inverted on some platform, flip the sign here (one-liner) rather than
        // in the pure step.
        ui.on_zoom_at(move |x, y, dy| {
            with_ui(&ui_weak, |ui| {
                viewport.borrow_mut().zoom_at(x, y, dy);
                apply_viewport(&ui, &viewport.borrow());
            })
        });
    }
    {
        let viewport = Rc::clone(&viewport);
        // Drag start: snapshot the current offset; no geometry change yet.
        ui.on_begin_pan(move || {
            viewport.borrow_mut().begin_pan();
        });
    }
    {
        let ui_weak = ui.as_weak();
        let viewport = Rc::clone(&viewport);
        ui.on_pan_to(move |dx, dy| {
            with_ui(&ui_weak, |ui| {
                viewport.borrow_mut().pan_to(dx, dy);
                apply_viewport(&ui, &viewport.borrow());
            })
        });
    }
}

/// Registers the keyboard navigation hub and window-resize callbacks onto `ui`.
/// Panel constraint (#151): explicit handle list IS the dependency list — no AppState bundle.
#[allow(clippy::too_many_arguments)]
pub(crate) fn wire_nav_handlers(
    ui: &ViewerWindow,
    state: &Rc<RefCell<ViewerState>>,
    viewport: &Rc<RefCell<ViewportState>>,
    settings: &Rc<RefCell<Settings>>,
    library: &Rc<RefCell<Library>>,
    nav: &Rc<RefCell<NavState>>,
    covers: &Rc<cover_loader::CoverController>,
    pages: &Rc<PageController>,
    search: &Rc<RefCell<LibrarySearchState>>,
    selection: &Rc<RefCell<LibrarySelectionState>>,
    localizer: &Rc<i18n::Localizer>,
) {
    let state = Rc::clone(state);
    let viewport = Rc::clone(viewport);
    let settings = Rc::clone(settings);
    let library = Rc::clone(library);
    let nav = Rc::clone(nav);
    let covers = Rc::clone(covers);
    let pages = Rc::clone(pages);
    let search = Rc::clone(search);
    let selection = Rc::clone(selection);
    let localizer = Rc::clone(localizer);

    // Keyboard navigation forwarded from the FocusScope.
    {
        let ui_weak = ui.as_weak();
        let state = Rc::clone(&state);
        let viewport = Rc::clone(&viewport);
        let nav = Rc::clone(&nav);
        let library = Rc::clone(&library);
        let pages = Rc::clone(&pages);
        let localizer = Rc::clone(&localizer);
        // `settings` is captured only to satisfy `persist_view_modes`'s signature
        // on the GoToLibrary leave point; the LeaveViewer route never reads it.
        let settings = Rc::clone(&settings);
        // The carousel-refresh collaborators are captured because the GoToLibrary
        // arm rebuilds the carousel on entry (continue-reading freshness +
        // focus snap) through `go_to_library` / `refresh_library_carousel`.
        let covers = Rc::clone(&covers);
        let search = Rc::clone(&search);
        let selection = Rc::clone(&selection);
        ui.on_nav(move |token| {
            with_ui(&ui_weak, |ui| {
                let dir = state.borrow().reading_direction();
                let Some(cmd) = map_key(token.as_str(), dir) else {
                    return;
                };
                let started = std::time::Instant::now();
                match cmd {
                    KeyCommand::Turn(action) => {
                        let moved = state.borrow_mut().apply(action);
                        if moved {
                            refresh(
                                &ui,
                                &state.borrow(),
                                &viewport,
                                localizer.loader(),
                                &pages,
                                ui.as_weak(),
                            );
                        }
                        // Log every page-turn latency (cache hits target <50ms; the
                        // first visit to a page also includes a synchronous decode).
                        // Observe with RUST_LOG=debug.
                        tracing::debug!(
                            elapsed_ms = started.elapsed().as_secs_f64() * 1000.0,
                            moved,
                            "page turn"
                        );
                        // Reveal the auto-hiding chrome on a page-turn key (spec §5).
                        ui.invoke_reveal_chrome_now();
                    }
                    // Runtime state is the single source of truth for these modes;
                    // `persist_view_modes` routes them into `Settings` or the
                    // per-book override at the next leave point (no per-key
                    // Settings write).
                    KeyCommand::ToggleSpread => {
                        if state.borrow_mut().toggle_spread() {
                            refresh(
                                &ui,
                                &state.borrow(),
                                &viewport,
                                localizer.loader(),
                                &pages,
                                ui.as_weak(),
                            );
                        }
                    }
                    KeyCommand::ToggleReadingDirection => {
                        if state.borrow_mut().toggle_reading_direction() {
                            refresh(
                                &ui,
                                &state.borrow(),
                                &viewport,
                                localizer.loader(),
                                &pages,
                                ui.as_weak(),
                            );
                        }
                    }
                    KeyCommand::ToggleCover => {
                        if state.borrow_mut().toggle_cover() {
                            refresh(
                                &ui,
                                &state.borrow(),
                                &viewport,
                                localizer.loader(),
                                &pages,
                                ui.as_weak(),
                            );
                        }
                    }
                    // Zoom/fit commands mutate `ViewportState`, then push geometry.
                    // The viewport owns `fit_mode` at runtime; `persist_view_modes`
                    // routes it into `Settings` or the per-book override at the
                    // next leave point (zoom/pan stay session-only). Each mutates
                    // in its own statement, then applies geometry with a fresh
                    // immutable borrow (never hold borrow_mut across apply).
                    KeyCommand::ZoomIn => {
                        viewport.borrow_mut().zoom_step(true);
                        apply_viewport(&ui, &viewport.borrow());
                    }
                    KeyCommand::ZoomOut => {
                        viewport.borrow_mut().zoom_step(false);
                        apply_viewport(&ui, &viewport.borrow());
                    }
                    KeyCommand::ResetView => {
                        viewport.borrow_mut().reset();
                        apply_viewport(&ui, &viewport.borrow());
                    }
                    // Fit changes reset zoom + re-center. The viewport owns `fit_mode`;
                    // `persist_view_modes` persists it at the next leave point
                    // (zoom/pan are NOT persisted — session-only).
                    KeyCommand::FitActual => {
                        viewport.borrow_mut().set_fit(FitMode::Actual);
                        apply_viewport(&ui, &viewport.borrow());
                    }
                    KeyCommand::CycleFit => {
                        viewport.borrow_mut().cycle_fit();
                        apply_viewport(&ui, &viewport.borrow());
                    }
                    // Toggle the thumbnail strip. No refresh needed: the strip's
                    // appearance changes PageView's height, which auto-fires the
                    // existing `viewport-resized` wiring.
                    KeyCommand::ToggleThumbnails => {
                        ui.set_show_thumbnails(!ui.get_show_thumbnails());
                    }
                    // Up arrow returns to the Library carousel. Direction-independent
                    // (decoded in keymap); the seam flips NavState + syncs `screen`.
                    KeyCommand::GoToLibrary => {
                        // Write the current position AND this book's view modes back
                        // before leaving the viewer, so a D/R/C/fit toggle made while
                        // reading persists to the book even without opening settings.
                        // View-mode routing goes through `persist_view_modes`
                        // (ADR-0007). Both confine their borrows to single statements,
                        // dropping before go_to_library borrows the UI.
                        write_back_position(&state, &library);
                        persist_view_modes(
                            ViewModeRoute::LeaveViewer,
                            &state,
                            &viewport,
                            &settings,
                            &library,
                        );
                        // `go_to_library` rebuilds the carousel on entry so the
                        // continue-reading ribbon reflects the `last_opened` just
                        // persisted above, and snaps focus to that book. The
                        // CarouselRefresh borrows are cheap `&Rc` references.
                        go_to_library(
                            &ui,
                            &nav,
                            &CarouselRefresh {
                                library: &library,
                                covers: &covers,
                                search: &search,
                                selection: &selection,
                                localizer: &localizer,
                            },
                        );
                    }
                }
            })
        });
    }

    // Viewport resize: re-resolve SpreadMode::Auto against the new window aspect;
    // refresh only when the effective layout actually flipped (no churn while
    // merely resizing in Single or Double mode).
    {
        let ui_weak = ui.as_weak();
        let state = Rc::clone(&state);
        let viewport = Rc::clone(&viewport);
        let pages = Rc::clone(&pages);
        let localizer = Rc::clone(&localizer);
        ui.on_resized(move |w, h| {
            with_ui(&ui_weak, |ui| {
                if state.borrow_mut().set_viewport_size(w, h) {
                    refresh(
                        &ui,
                        &state.borrow(),
                        &viewport,
                        localizer.loader(),
                        &pages,
                        ui.as_weak(),
                    );
                }
            })
        });
    }
}
