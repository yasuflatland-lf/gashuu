slint::include_modules!();

mod keymap;
mod viewer_state;
mod viewport;

use gashuu_core::{
    generate_thumbnails, CoreError, CoverMode, DecodedImage, FitMode, ReadingDirection, Settings,
    SpreadMode, DEFAULT_THUMB_MAX_SIDE,
};
use keymap::{map_key, KeyCommand};
use slint::{Model, ModelRc, VecModel};
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering::Relaxed};
use std::sync::Arc;
use viewer_state::ViewerState;
use viewport::ViewportState;

fn main() -> color_eyre::Result<()> {
    color_eyre::install()?;
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    // Load persisted settings; corrupt/unreadable files fall back to defaults
    // (the corrupt-file recovery policy lives here in the UI layer, by design).
    let settings = Settings::load().unwrap_or_else(|e| {
        tracing::warn!(error = %e, "failed to load settings; using defaults");
        Settings::default()
    });

    let ui = ViewerWindow::new()?;
    let state = Rc::new(RefCell::new(ViewerState::from_settings(&settings)));
    let viewport = Rc::new(RefCell::new(ViewportState::from_settings(&settings)));
    let settings = Rc::new(RefCell::new(settings));

    // Thumbnail-strip model + generation bookkeeping. The model is the live
    // backing store for the strip (placeholders first, filled in as decodes
    // complete on a background thread). `thumb_epoch` tags each generation so a
    // superseded book's late callbacks are dropped; `thumb_cancel` lets us flip
    // the previous generation's cancel flag the instant a new book opens.
    let thumb_model = Rc::new(VecModel::<ThumbnailItem>::default());
    ui.set_thumbnails(ModelRc::from(thumb_model.clone()));
    let thumb_epoch = Arc::new(AtomicUsize::new(0));
    let thumb_cancel = Rc::new(RefCell::new(Arc::new(AtomicBool::new(false))));

    // Launch a fresh parallel thumbnail generation for the currently open
    // source. Runs on the UI thread: it cancels the previous generation, resets
    // the model to N placeholders, then spawns a worker that streams decoded
    // thumbnails back via `invoke_from_event_loop`. Invoked after every
    // successful open.
    let start_thumbnails = {
        let ui_weak = ui.as_weak();
        let state = Rc::clone(&state);
        let thumb_cancel = Rc::clone(&thumb_cancel);
        let thumb_epoch = Arc::clone(&thumb_epoch);
        let thumb_model = Rc::clone(&thumb_model);
        move || {
            // 1. Cancel any in-flight generation, then install a fresh flag.
            //    Two-statement borrow discipline: the temporary `borrow()` drops
            //    at the `;` before we take `borrow_mut()`.
            thumb_cancel.borrow().store(true, Relaxed);
            *thumb_cancel.borrow_mut() = Arc::new(AtomicBool::new(false));
            let cancel_flag = Arc::clone(&thumb_cancel.borrow());

            // 2. Tag this generation so superseded callbacks can be dropped.
            let my_epoch = thumb_epoch.fetch_add(1, Relaxed) + 1;

            // 3. Rebuild the model with N gray placeholders (loaded = false).
            let page_count = state.borrow().page_count();
            let placeholders: Vec<ThumbnailItem> = (0..page_count)
                .map(|i| ThumbnailItem {
                    image: Default::default(),
                    page: i as i32,
                    loaded: false,
                    failed: false,
                })
                .collect();
            thumb_model.set_vec(placeholders);

            // 4. Grab the source (None => nothing to generate).
            let source = match state.borrow().current_source() {
                Some(s) => s,
                None => return,
            };

            // 5. Spawn the worker. `on_ready` may capture only Send values: the
            //    Weak (Send+Sync), the epoch Arc, and the epoch usize. It must
            //    NOT capture the Rc model nor build a slint::Image off-thread
            //    (both non-Send) — the cell is built inside the event loop.
            let weak = ui_weak.clone();
            let epoch = Arc::clone(&thumb_epoch);
            let on_ready = move |i: usize, res: Result<DecodedImage, CoreError>| {
                let img = match res {
                    Ok(img) => img,
                    Err(e) => {
                        // Log the error on the worker thread before marshaling so
                        // CoreError never crosses the thread boundary.
                        tracing::warn!(page = i, error = %e, "thumbnail generation failed");
                        // Marshal a failed-cell update to the UI thread so the
                        // cell shows a distinct red placeholder instead of the
                        // indistinguishable gray loading state.
                        let weak = weak.clone();
                        let epoch = Arc::clone(&epoch);
                        if let Err(e) = slint::invoke_from_event_loop(move || {
                            // Drop results from a superseded generation.
                            if epoch.load(Relaxed) != my_epoch {
                                return;
                            }
                            let Some(ui) = weak.upgrade() else {
                                return;
                            };
                            let model = ui.get_thumbnails();
                            let Some(vm) = model.as_any().downcast_ref::<VecModel<ThumbnailItem>>()
                            else {
                                return;
                            };
                            if i < vm.row_count() {
                                vm.set_row_data(
                                    i,
                                    ThumbnailItem {
                                        image: Default::default(),
                                        page: i as i32,
                                        loaded: false,
                                        failed: true,
                                    },
                                );
                            }
                        }) {
                            tracing::debug!(
                                page = i,
                                error = %e,
                                "dropped failed-thumbnail update; event loop gone"
                            );
                        }
                        return;
                    }
                };
                let weak = weak.clone();
                let epoch = Arc::clone(&epoch);
                if let Err(e) = slint::invoke_from_event_loop(move || {
                    // Drop results from a superseded generation.
                    if epoch.load(Relaxed) != my_epoch {
                        return;
                    }
                    let Some(ui) = weak.upgrade() else {
                        return;
                    };
                    // Re-fetch the model on the UI thread (never move the Rc
                    // across threads); convert to slint::Image here (non-Send).
                    let model = ui.get_thumbnails();
                    let Some(vm) = model.as_any().downcast_ref::<VecModel<ThumbnailItem>>() else {
                        return;
                    };
                    if i < vm.row_count() {
                        vm.set_row_data(
                            i,
                            ThumbnailItem {
                                image: to_slint_image(&img),
                                page: i as i32,
                                loaded: true,
                                failed: false,
                            },
                        );
                    }
                }) {
                    tracing::debug!(page = i, error = %e, "dropped thumbnail update; event loop gone");
                }
            };
            std::thread::spawn(move || {
                generate_thumbnails(source, DEFAULT_THUMB_MAX_SIDE, cancel_flag, on_ready);
            });
        }
    };
    // Wrap so both open handlers can share the single generator closure.
    let start_thumbnails = Rc::new(start_thumbnails);

    // Initial paint so rtl/single/status are all initialized before the first
    // folder is opened (refresh shows "No folder opened" and clears the images).
    refresh(&ui, &state.borrow(), &viewport);

    // First-run guide: show the overlay exactly once. `seen_guide` is flipped and
    // persisted when the user dismisses it (see `on_dismiss_guide`).
    if !settings.borrow().seen_guide {
        ui.set_show_guide(true);
    }

    // Open Folder button: pick a directory, load it, refresh the view.
    {
        let ui_weak = ui.as_weak();
        let state = Rc::clone(&state);
        let settings = Rc::clone(&settings);
        let viewport = Rc::clone(&viewport);
        let start_thumbnails = Rc::clone(&start_thumbnails);
        ui.on_open_folder(move || {
            let Some(ui) = ui_weak.upgrade() else {
                return;
            };
            let Some(dir) = rfd::FileDialog::new().pick_folder() else {
                return;
            };
            match state.borrow_mut().open_folder(&dir) {
                Ok(()) => {
                    tracing::info!(dir = %dir.display(), "opened folder");
                    let mut s = settings.borrow_mut();
                    if s.track_recent_files {
                        s.push_recent(dir.clone());
                        if let Err(e) = s.save() {
                            tracing::error!(error = %e, "failed to save settings");
                        }
                    }
                }
                Err(e) => {
                    tracing::error!(error = %e, "failed to open folder");
                    ui.set_status_text(format!("Error: {e}").into());
                    return;
                }
            }
            refresh(&ui, &state.borrow(), &viewport);
            let skipped = state.borrow().last_open_skipped();
            if skipped > 0 {
                let base = ui.get_status_text().to_string();
                ui.set_status_text(format!("{base} \u{2014} {skipped} entries skipped").into());
            }
            // Kick off parallel thumbnail generation for the newly opened source.
            start_thumbnails();
        });
    }

    // Open Archive button: pick a CBZ/ZIP/CBR/RAR file, load it, refresh the view.
    {
        let ui_weak = ui.as_weak();
        let state = Rc::clone(&state);
        let settings = Rc::clone(&settings);
        let viewport = Rc::clone(&viewport);
        let start_thumbnails = Rc::clone(&start_thumbnails);
        ui.on_open_archive(move || {
            let Some(ui) = ui_weak.upgrade() else {
                return;
            };
            let Some(file) = rfd::FileDialog::new()
                .add_filter("Comic archive", &["cbz", "zip", "cbr", "rar"])
                .pick_file()
            else {
                return;
            };
            match state.borrow_mut().open_path(&file) {
                Ok(()) => {
                    tracing::info!(path = %file.display(), "opened archive");
                    let mut s = settings.borrow_mut();
                    if s.track_recent_files {
                        s.push_recent(file.clone());
                        if let Err(e) = s.save() {
                            tracing::error!(error = %e, "failed to save settings");
                        }
                    }
                }
                Err(e) => {
                    tracing::error!(error = %e, "failed to open archive");
                    ui.set_status_text(format!("Error: {e}").into());
                    return;
                }
            }
            refresh(&ui, &state.borrow(), &viewport);
            let skipped = state.borrow().last_open_skipped();
            if skipped > 0 {
                let base = ui.get_status_text().to_string();
                ui.set_status_text(
                    format!("{base} \u{2014} {skipped} entries skipped (zip-slip or oversized)")
                        .into(),
                );
            }
            // Kick off parallel thumbnail generation for the newly opened source.
            start_thumbnails();
        });
    }

    // Thumbnail click: jump to the clicked page's spread, refresh, then restore
    // focus to the page area so keyboard navigation keeps working.
    {
        let ui_weak = ui.as_weak();
        let state = Rc::clone(&state);
        let viewport = Rc::clone(&viewport);
        ui.on_thumbnail_clicked(move |page| {
            let Some(ui) = ui_weak.upgrade() else {
                return;
            };
            if state.borrow_mut().jump_to(page as usize) {
                refresh(&ui, &state.borrow(), &viewport);
            }
            ui.invoke_focus_pages();
        });
    }

    // Toggle the thumbnail strip's visibility. No refresh needed: showing/hiding
    // the strip changes PageView's height, which fires the existing
    // `viewport-resized` wiring automatically.
    {
        let ui_weak = ui.as_weak();
        ui.on_toggle_thumbnails(move || {
            let Some(ui) = ui_weak.upgrade() else {
                return;
            };
            ui.set_show_thumbnails(!ui.get_show_thumbnails());
        });
    }

    // Open the settings dialog: push the current persisted values into the
    // dialog's in-out properties, then show it. A single immutable borrow of
    // `settings` covers all reads (avoids a double-borrow).
    {
        let ui_weak = ui.as_weak();
        let settings = Rc::clone(&settings);
        let viewport = Rc::clone(&viewport);
        ui.on_open_settings(move || {
            let Some(ui) = ui_weak.upgrade() else {
                return;
            };
            let s = settings.borrow();
            ui.set_reading_direction_index(reading_direction_to_index(s.reading_direction));
            ui.set_spread_mode_index(spread_mode_to_index(s.spread_mode));
            ui.set_cover_mode_index(cover_mode_to_index(s.cover_mode));
            // Fit mode is owned by the viewport at runtime; the persisted value in
            // `settings` mirrors it (kept in sync by the fit handlers), so read it
            // from the live viewport to be authoritative.
            ui.set_fit_mode_index(fit_mode_to_index(viewport.borrow().fit_mode()));
            ui.set_cache_size(s.cache_size as i32);
            ui.set_preload_pages(s.preload_pages as i32);
            ui.set_track_recent(s.track_recent_files);
            ui.set_key_bindings_text(KEY_BINDINGS_HELP.into());
            ui.set_show_settings(true);
        });
    }

    // Close the settings dialog: hide it, persist settings, then restore focus to
    // the page area so keyboard navigation keeps working.
    {
        let ui_weak = ui.as_weak();
        let settings = Rc::clone(&settings);
        ui.on_close_settings(move || {
            let Some(ui) = ui_weak.upgrade() else {
                return;
            };
            ui.set_show_settings(false);
            if let Err(e) = settings.borrow().save() {
                tracing::error!(error = %e, "failed to save settings from dialog");
                ui.set_status_text(format!("Could not save settings: {e}").into());
            }
            ui.invoke_focus_pages();
        });
    }

    // Dismiss the first-run guide: mark it seen, persist, hide it, restore focus.
    // Two-statement RefCell discipline: the `borrow_mut()` drops at the `;` before
    // the immutable `borrow()` for `save`.
    {
        let ui_weak = ui.as_weak();
        let settings = Rc::clone(&settings);
        ui.on_dismiss_guide(move || {
            let Some(ui) = ui_weak.upgrade() else {
                return;
            };
            // Persist immediately; a persistent save failure here is non-fatal — the
            // guide simply re-shows next launch (seen_guide is also saved on exit).
            settings.borrow_mut().seen_guide = true;
            if let Err(e) = settings.borrow().save() {
                tracing::error!(error = %e, "failed to save settings on guide dismiss");
            }
            ui.set_show_guide(false);
            ui.invoke_focus_pages();
        });
    }

    // Settings setters (one per dialog control). Mode/cover/direction are made
    // idempotent by their `ViewerState` value setters returning a "changed" bool;
    // fit uses an explicit equality guard. The borrow discipline mirrors the
    // `ToggleSpread` handler: the temporary `borrow_mut()` in the `if` condition
    // drops before the block runs, so `refresh(&ui, &state.borrow(), ..)` is safe.
    {
        let ui_weak = ui.as_weak();
        let state = Rc::clone(&state);
        let settings = Rc::clone(&settings);
        let viewport = Rc::clone(&viewport);
        ui.on_set_reading_direction(move |i| {
            let Some(ui) = ui_weak.upgrade() else {
                return;
            };
            let dir = index_to_reading_direction(i);
            if state.borrow_mut().set_reading_direction(dir) {
                settings.borrow_mut().reading_direction = dir;
                refresh(&ui, &state.borrow(), &viewport);
            }
        });
    }
    {
        let ui_weak = ui.as_weak();
        let state = Rc::clone(&state);
        let settings = Rc::clone(&settings);
        let viewport = Rc::clone(&viewport);
        ui.on_set_spread_mode(move |i| {
            let Some(ui) = ui_weak.upgrade() else {
                return;
            };
            let mode = index_to_spread_mode(i);
            if state.borrow_mut().set_spread_mode(mode) {
                settings.borrow_mut().spread_mode = mode;
                refresh(&ui, &state.borrow(), &viewport);
            }
        });
    }
    {
        let ui_weak = ui.as_weak();
        let state = Rc::clone(&state);
        let settings = Rc::clone(&settings);
        let viewport = Rc::clone(&viewport);
        ui.on_set_cover_mode(move |i| {
            let Some(ui) = ui_weak.upgrade() else {
                return;
            };
            let mode = index_to_cover_mode(i);
            if state.borrow_mut().set_cover_mode(mode) {
                settings.borrow_mut().cover_mode = mode;
                refresh(&ui, &state.borrow(), &viewport);
            }
        });
    }
    {
        let ui_weak = ui.as_weak();
        let state = Rc::clone(&state);
        let settings = Rc::clone(&settings);
        let viewport = Rc::clone(&viewport);
        ui.on_set_fit_mode(move |i| {
            let Some(ui) = ui_weak.upgrade() else {
                return;
            };
            let mode = index_to_fit_mode(i);
            // Equality guard (the viewport setter is not idempotent-by-return).
            // Compare in one borrow, mutate in a separate `borrow_mut()` that
            // drops at the `;`, then `refresh` (which borrows viewport internally).
            if viewport.borrow().fit_mode() != mode {
                viewport.borrow_mut().set_fit(mode);
                settings.borrow_mut().fit_mode = mode;
                refresh(&ui, &state.borrow(), &viewport);
            }
        });
    }
    {
        let state = Rc::clone(&state);
        let settings = Rc::clone(&settings);
        // Cache size applies to newly opened books; no refresh of the current view.
        ui.on_set_cache_size(move |v| {
            let v = (v.max(1)) as usize;
            // Read the current preload while writing cache_size, then mirror both
            // into ViewerState so the next opened book picks up the change this
            // session.
            let preload = {
                let mut s = settings.borrow_mut();
                s.cache_size = v;
                s.preload_pages
            };
            state.borrow_mut().set_cache_config(v, preload);
        });
    }
    {
        let state = Rc::clone(&state);
        let settings = Rc::clone(&settings);
        // Preload radius applies to newly opened books; no refresh. 0 is a valid
        // "prefetch disabled" radius, so only clamp the negative tail.
        ui.on_set_preload_pages(move |v| {
            let v = (v.max(0)) as usize;
            let cache_size = {
                let mut s = settings.borrow_mut();
                s.preload_pages = v;
                s.cache_size
            };
            state.borrow_mut().set_cache_config(cache_size, v);
        });
    }
    {
        let settings = Rc::clone(&settings);
        ui.on_set_track_recent(move |b| {
            settings.borrow_mut().track_recent_files = b;
        });
    }

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
            let Some(ui) = ui_weak.upgrade() else {
                return;
            };
            viewport.borrow_mut().resize(w, h);
            apply_viewport(&ui, &viewport.borrow());
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
            let Some(ui) = ui_weak.upgrade() else {
                return;
            };
            viewport.borrow_mut().zoom_at(x, y, dy);
            apply_viewport(&ui, &viewport.borrow());
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
            let Some(ui) = ui_weak.upgrade() else {
                return;
            };
            viewport.borrow_mut().pan_to(dx, dy);
            apply_viewport(&ui, &viewport.borrow());
        });
    }

    // Keyboard navigation forwarded from the FocusScope.
    {
        let ui_weak = ui.as_weak();
        let state = Rc::clone(&state);
        let settings = Rc::clone(&settings);
        let viewport = Rc::clone(&viewport);
        ui.on_nav(move |token| {
            let Some(ui) = ui_weak.upgrade() else {
                return;
            };
            let dir = state.borrow().reading_direction();
            let Some(cmd) = map_key(token.as_str(), dir) else {
                return;
            };
            let started = std::time::Instant::now();
            match cmd {
                KeyCommand::Turn(action) => {
                    let moved = state.borrow_mut().apply(action);
                    if moved {
                        refresh(&ui, &state.borrow(), &viewport);
                    }
                    // Log every page-turn latency (cache hits target <50ms; the
                    // first visit to a page also includes a synchronous decode).
                    // Observe with RUST_LOG=debug.
                    tracing::debug!(
                        elapsed_ms = started.elapsed().as_secs_f64() * 1000.0,
                        moved,
                        "page turn"
                    );
                }
                KeyCommand::ToggleSpread => {
                    if state.borrow_mut().toggle_spread() {
                        settings.borrow_mut().spread_mode = state.borrow().spread_mode();
                        refresh(&ui, &state.borrow(), &viewport);
                    }
                }
                KeyCommand::ToggleReadingDirection => {
                    if state.borrow_mut().toggle_reading_direction() {
                        settings.borrow_mut().reading_direction =
                            state.borrow().reading_direction();
                        refresh(&ui, &state.borrow(), &viewport);
                    }
                }
                KeyCommand::ToggleCover => {
                    if state.borrow_mut().toggle_cover() {
                        settings.borrow_mut().cover_mode = state.borrow().cover_mode();
                        refresh(&ui, &state.borrow(), &viewport);
                    }
                }
                // Zoom/fit commands mutate `ViewportState`, then push geometry;
                // `fit_mode` is reflected into `Settings` below (zoom/pan stay
                // session-only). Each mutates in its own statement, then applies
                // geometry with a fresh immutable borrow (never hold borrow_mut
                // across apply).
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
                // Fit changes reset zoom + re-center; reflect the new fit_mode into
                // the in-memory Settings so save-on-exit persists it (zoom/pan are
                // NOT persisted — session-only).
                KeyCommand::FitActual => {
                    viewport.borrow_mut().set_fit(FitMode::Actual);
                    settings.borrow_mut().fit_mode = viewport.borrow().fit_mode();
                    apply_viewport(&ui, &viewport.borrow());
                }
                KeyCommand::CycleFit => {
                    viewport.borrow_mut().cycle_fit();
                    settings.borrow_mut().fit_mode = viewport.borrow().fit_mode();
                    apply_viewport(&ui, &viewport.borrow());
                }
                // Toggle the thumbnail strip. No refresh needed: the strip's
                // appearance changes PageView's height, which auto-fires the
                // existing `viewport-resized` wiring.
                KeyCommand::ToggleThumbnails => {
                    ui.set_show_thumbnails(!ui.get_show_thumbnails());
                }
            }
        });
    }

    // Viewport resize: re-resolve SpreadMode::Auto against the new window aspect;
    // refresh only when the effective layout actually flipped (no churn while
    // merely resizing in Single or Double mode).
    {
        let ui_weak = ui.as_weak();
        let state = Rc::clone(&state);
        let viewport = Rc::clone(&viewport);
        ui.on_resized(move |w, h| {
            let Some(ui) = ui_weak.upgrade() else {
                return; // window is being torn down
            };
            if state.borrow_mut().set_viewport_size(w, h) {
                refresh(&ui, &state.borrow(), &viewport);
            }
        });
    }

    ui.run()?;
    // Persist settings on exit so even a first run writes a file the user can
    // hand-edit. Save failure is logged, not fatal.
    if let Err(e) = settings.borrow().save() {
        tracing::error!(error = %e, "failed to save settings on exit");
    }
    Ok(())
}

/// Push the current spread + status into the UI, then re-anchor the viewport to
/// the new content size and push the resulting geometry.
fn refresh(ui: &ViewerWindow, state: &ViewerState, viewport: &Rc<RefCell<ViewportState>>) {
    let status = state.status_text();
    ui.set_rtl(matches!(state.reading_direction(), ReadingDirection::Rtl));
    match state.current_spread() {
        Some(Ok(spread)) => {
            // Content pixel size for the viewport: single page = the leading
            // page; double page = widths summed side-by-side, height = the taller
            // of the two. Compute before swapping the images so the viewport
            // re-centers for the new spread.
            let (content_w, content_h) = match &spread.trailing {
                // Each page gets an equal half of content-w (horizontal-stretch 1:1 in PageView.slint);
                // contain-fit within that half-slot letterboxes a wide page or pillarboxes a tall one.
                // Exact for equal-size manga pages.
                Some(trailing) => (
                    (spread.leading.width() + trailing.width()) as f32,
                    spread.leading.height().max(trailing.height()) as f32,
                ),
                None => (
                    spread.leading.width() as f32,
                    spread.leading.height() as f32,
                ),
            };
            ui.set_leading_page(to_slint_image(&spread.leading));
            match spread.trailing {
                Some(trailing) => {
                    ui.set_trailing_page(to_slint_image(&trailing));
                    ui.set_single(false);
                }
                None => {
                    ui.set_trailing_page(slint::Image::default());
                    ui.set_single(true);
                }
            }
            // A trailing-page decode failure degraded the view to leading-only;
            // append a marker so the status no longer contradicts the single
            // page actually shown.
            match spread.trailing_failed {
                Some(failed) => ui
                    .set_status_text(format!("{status}  (page {} unavailable)", failed + 1).into()),
                None => ui.set_status_text(status.into()),
            }
            // Re-anchor the viewport to the new content, then push geometry.
            // Borrow-scoping rule: never hold a `borrow_mut()` while constructing
            // the `&viewport.borrow()` argument to `apply_viewport`. Mutate in one
            // statement (the temporary `borrow_mut` drops at the `;`), then take a
            // fresh immutable borrow for apply.
            viewport.borrow_mut().set_content(content_w, content_h);
            apply_viewport(ui, &viewport.borrow());
        }
        Some(Err(e)) => {
            tracing::error!(error = %e, "failed to decode page");
            ui.set_status_text(format!("Decode error: {e}").into());
            ui.set_leading_page(slint::Image::default());
            ui.set_trailing_page(slint::Image::default());
            ui.set_single(true);
            // No valid content: zero the content size so geometry collapses to an
            // empty box (the page is already cleared above — keep the view
            // consistent rather than retaining a stale content rectangle).
            viewport.borrow_mut().set_content(0.0, 0.0);
            apply_viewport(ui, &viewport.borrow());
        }
        None => {
            // Source loaded but empty (or no source yet): clear and show single
            // so the view matches the status text ("No folder opened" / "Folder
            // contains no images").
            ui.set_status_text(status.into());
            ui.set_leading_page(slint::Image::default());
            ui.set_trailing_page(slint::Image::default());
            ui.set_single(true);
            viewport.borrow_mut().set_content(0.0, 0.0);
            apply_viewport(ui, &viewport.borrow());
        }
    }
    // Keep the thumbnail-strip highlight in sync with the current spread's
    // leading page after every navigation/refresh.
    ui.set_current_index(state.index() as i32);
}

/// Push the viewport's render geometry (content_x/y/w/h, logical px as `f32`)
/// into the UI properties.
fn apply_viewport(ui: &ViewerWindow, viewport: &ViewportState) {
    let (x, y, w, h) = viewport.geometry();
    ui.set_content_x(x);
    ui.set_content_y(y);
    ui.set_content_w(w);
    ui.set_content_h(h);
}

/// Convert core RGBA bytes into a `slint::Image`.
fn to_slint_image(decoded: &DecodedImage) -> slint::Image {
    let mut buffer =
        slint::SharedPixelBuffer::<slint::Rgba8Pixel>::new(decoded.width(), decoded.height());
    buffer.make_mut_bytes().copy_from_slice(decoded.rgba());
    slint::Image::from_rgba8(buffer)
}

/// Concise key-bindings reference shown read-only in the settings dialog. Keep in
/// sync with `keymap::map_key`.
const KEY_BINDINGS_HELP: &str = "\
Navigation:
  Space = next page    Backspace = previous page
  Arrows follow the reading direction (LTR: \u{2192} next; RTL: \u{2190} next)

Modes:
  D = spread (single \u{2192} double \u{2192} auto)
  R = reading direction (LTR / RTL)
  C = cover layout (standalone / paired)

Zoom & fit:
  + / - = zoom in / out    0 = reset view    1 = actual size    f = cycle fit

View:
  T = toggle thumbnail strip";

// Enum <-> ComboBox index conversions. `*_to_index` use exhaustive matches so a
// new enum variant becomes a compile error; `index_to_*` default to the first
// variant for any out-of-range index Slint may send (the index is a raw i32).
// The ordering is authoritative and MUST match the ComboBox model order in
// SettingsDialog.slint:
//   ReadingDirection: Ltr=0, Rtl=1
//   SpreadMode:       Single=0, Double=1, Auto=2
//   CoverMode:        Standalone=0, Paired=1
//   FitMode:          Whole=0, Width=1, Actual=2

fn reading_direction_to_index(d: ReadingDirection) -> i32 {
    match d {
        ReadingDirection::Ltr => 0,
        ReadingDirection::Rtl => 1,
    }
}

fn index_to_reading_direction(i: i32) -> ReadingDirection {
    match i {
        1 => ReadingDirection::Rtl,
        _ => ReadingDirection::Ltr,
    }
}

fn spread_mode_to_index(m: SpreadMode) -> i32 {
    match m {
        SpreadMode::Single => 0,
        SpreadMode::Double => 1,
        SpreadMode::Auto => 2,
    }
}

fn index_to_spread_mode(i: i32) -> SpreadMode {
    match i {
        1 => SpreadMode::Double,
        2 => SpreadMode::Auto,
        _ => SpreadMode::Single,
    }
}

fn cover_mode_to_index(m: CoverMode) -> i32 {
    match m {
        CoverMode::Standalone => 0,
        CoverMode::Paired => 1,
    }
}

fn index_to_cover_mode(i: i32) -> CoverMode {
    match i {
        1 => CoverMode::Paired,
        _ => CoverMode::Standalone,
    }
}

fn fit_mode_to_index(m: FitMode) -> i32 {
    match m {
        FitMode::Whole => 0,
        FitMode::Width => 1,
        FitMode::Actual => 2,
    }
}

fn index_to_fit_mode(i: i32) -> FitMode {
    match i {
        1 => FitMode::Width,
        2 => FitMode::Actual,
        _ => FitMode::Whole,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reading_direction_index_round_trips() {
        for d in [ReadingDirection::Ltr, ReadingDirection::Rtl] {
            assert_eq!(index_to_reading_direction(reading_direction_to_index(d)), d);
        }
    }

    #[test]
    fn spread_mode_index_round_trips() {
        for m in [SpreadMode::Single, SpreadMode::Double, SpreadMode::Auto] {
            assert_eq!(index_to_spread_mode(spread_mode_to_index(m)), m);
        }
    }

    #[test]
    fn cover_mode_index_round_trips() {
        for m in [CoverMode::Standalone, CoverMode::Paired] {
            assert_eq!(index_to_cover_mode(cover_mode_to_index(m)), m);
        }
    }

    #[test]
    fn fit_mode_index_round_trips() {
        for m in [FitMode::Whole, FitMode::Width, FitMode::Actual] {
            assert_eq!(index_to_fit_mode(fit_mode_to_index(m)), m);
        }
    }

    #[test]
    fn out_of_range_indices_clamp_to_first_variant() {
        for bad in [-1, 3, 99, i32::MIN, i32::MAX] {
            assert_eq!(index_to_reading_direction(bad), ReadingDirection::Ltr);
            assert_eq!(index_to_spread_mode(bad), SpreadMode::Single);
            assert_eq!(index_to_cover_mode(bad), CoverMode::Standalone);
            assert_eq!(index_to_fit_mode(bad), FitMode::Whole);
        }
    }
}
