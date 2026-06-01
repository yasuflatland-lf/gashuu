slint::include_modules!();

mod keymap;
mod library_model;
mod navigation;
mod thumbnail_strip;
mod viewer_state;
mod viewport;

use gashuu_core::{
    CoverMode, DecodedImage, FitMode, Library, ReadingDirection, Settings, SpreadMode,
};
use keymap::{map_key, KeyCommand};
use library_model::{carousel_data, CarouselData};
use navigation::{screen_to_index, NavState};
use slint::{ModelRc, VecModel};
use std::cell::RefCell;
use std::path::Path;
use std::rc::Rc;
use thumbnail_strip::ThumbnailController;
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

    // Load the persisted shelf; an unreadable/corrupt library.json falls back
    // to an empty library (the corrupt-file recovery policy lives here in the
    // UI layer, mirroring the Settings load above).
    let library = Library::load().unwrap_or_else(|e| {
        tracing::warn!(error = %e, "failed to load library; starting empty");
        Library::new()
    });

    let ui = ViewerWindow::new()?;
    let state = Rc::new(RefCell::new(ViewerState::from_settings(&settings)));
    let viewport = Rc::new(RefCell::new(ViewportState::from_settings(&settings)));
    let settings = Rc::new(RefCell::new(settings));

    // The persisted shelf, shared so the carousel model build, the focused-index
    // clamp, and later PR-L's add / PR-R's position write-back can all reach it.
    let library = Rc::new(RefCell::new(library));

    // Thumbnail-strip controller. Owns the strip's backing model and the
    // generation bookkeeping (epoch + cancel double-guard); its `new` binds the
    // model into the UI via `set_thumbnails` internally. Wrapped in `Rc` so both
    // open handlers (via `open_and_present`) can share the single controller.
    let thumbs = Rc::new(ThumbnailController::new(&ui));

    // Held for PR-V (cover streaming) / PR-L (refresh after add) to mutate the
    // SAME model; not mutated by PR-C after the initial build. Underscore-bound
    // to satisfy -D warnings until those PRs use it.
    let _carousel_model = build_carousel_model(&ui, &library.borrow());
    ui.set_carousel_focused_index(0);

    // Top-level screen state machine. App boots to Library (the carousel home).
    // Held in an Rc<RefCell<…>> so the carousel callbacks and the Viewer's
    // GoToLibrary key arm can all flip it through the seam functions below.
    let nav = Rc::new(RefCell::new(NavState::new()));
    // Push the initial screen so the window shows the Library on boot.
    ui.set_screen(screen_to_index(nav.borrow().screen()));

    // Initial paint so rtl/single/status are all initialized before the first
    // folder is opened (refresh shows "No folder opened" and clears the images).
    refresh(&ui, &state.borrow(), &viewport);

    // First-run guide: show the overlay exactly once. `seen_guide` is flipped and
    // persisted when the user dismisses it (see `on_dismiss_guide`).
    if !settings.borrow().seen_guide {
        ui.set_show_guide(true);
    }

    // Open Folder button: pick a directory, open it, refresh the view, and start thumbnail generation.
    {
        let ui_weak = ui.as_weak();
        let state = Rc::clone(&state);
        let settings = Rc::clone(&settings);
        let viewport = Rc::clone(&viewport);
        let thumbs = Rc::clone(&thumbs);
        ui.on_open_folder(move || {
            let Some(ui) = ui_weak.upgrade() else {
                return;
            };
            let Some(dir) = rfd::FileDialog::new().pick_folder() else {
                return;
            };
            open_and_present(&ui, &state, &settings, &viewport, &thumbs, &dir, "");
        });
    }

    // Open Archive button: pick a CBZ/ZIP/CBR/RAR file, open it, refresh the view, and start thumbnail generation.
    {
        let ui_weak = ui.as_weak();
        let state = Rc::clone(&state);
        let settings = Rc::clone(&settings);
        let viewport = Rc::clone(&viewport);
        let thumbs = Rc::clone(&thumbs);
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
            open_and_present(
                &ui,
                &state,
                &settings,
                &viewport,
                &thumbs,
                &file,
                " (zip-slip or oversized)",
            );
        });
    }

    // Carousel: Return on the focused book opens it. PR-C delivers the correct
    // focused index (the move clamp keeps it in shelf bounds); resolving that
    // index to a Library book + opening/resuming its source is PR-R/PR-V. For
    // now it transitions to the Viewer so the seam is exercised end-to-end.
    {
        let ui_weak = ui.as_weak();
        let nav = Rc::clone(&nav);
        ui.on_carousel_open(move |_index| {
            let Some(ui) = ui_weak.upgrade() else {
                return;
            };
            go_to_viewer(&ui, &nav);
        });
    }
    // Carousel: Left/Right move the focused cover by `delta` (-1 / +1). Clamp
    // into the shelf bounds — the row ends are hard stops (no wrap), so Left on
    // the first book and Right on the last are inert. An empty shelf is a no-op
    // (no books to move between). focused-index is the single source of truth
    // for which cover is centered + which book Return opens.
    {
        let ui_weak = ui.as_weak();
        let library = Rc::clone(&library);
        ui.on_carousel_move(move |delta| {
            let Some(ui) = ui_weak.upgrade() else {
                return;
            };
            let count = library.borrow().books().len();
            if count == 0 {
                return; // empty shelf: nothing to move
            }
            let last = (count - 1) as i32;
            let next = (ui.get_carousel_focused_index() + delta).clamp(0, last);
            ui.set_carousel_focused_index(next);
        });
    }
    // Carousel: Down returns to the currently-open book (the Viewer). Only
    // meaningful when a book is open; with no book open it still flips to the
    // Viewer screen, which shows the "No folder opened" empty state (consistent
    // with the existing initial view). PR-R refines the "only when a book is
    // open" guard.
    {
        let ui_weak = ui.as_weak();
        let nav = Rc::clone(&nav);
        ui.on_carousel_back(move || {
            let Some(ui) = ui_weak.upgrade() else {
                return;
            };
            go_to_viewer(&ui, &nav);
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

    // Reveal the auto-hiding chrome and re-arm its idle-fade countdown. Fired on
    // mouse-move over the page and on a scrubber drag (arrow turns reveal via the
    // `nav` handler below).
    {
        let ui_weak = ui.as_weak();
        ui.on_reveal_chrome(move || {
            let Some(ui) = ui_weak.upgrade() else {
                return;
            };
            ui.invoke_reveal_chrome_now();
        });
    }

    // Drag preview: update ONLY the popover thumbnails + counter for the page
    // under the knob. The page body is unchanged until commit (spec decision 11).
    // Thumbnails are pulled from the EXISTING PR8a VecModel<ThumbnailItem> by page
    // index — no new decode, UI thread only (the Rc model is never crossed).
    {
        let ui_weak = ui.as_weak();
        let state = Rc::clone(&state);
        ui.on_scrub_preview(move |page| {
            let Some(ui) = ui_weak.upgrade() else {
                return;
            };
            let total = state.borrow().page_count();
            if total == 0 {
                return;
            }
            let lead = (page.max(0) as usize).min(total - 1);
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
            // Pull thumbnail images from the existing model (no decode).
            let model = ui.get_thumbnails();
            ui.set_scrubber_preview_a(thumb_image_at(&model, lead));
            ui.set_scrubber_preview_b(match trail {
                Some(trail) => thumb_image_at(&model, trail),
                None => slint::Image::default(),
            });
            // Update the counter to the previewed page (1-based).
            let counter = match trail {
                Some(trail) => format!("{}\u{2013}{} / {}", lead + 1, trail + 1, total),
                None => format!("{} / {}", lead + 1, total),
            };
            ui.set_page_counter_text(counter.into());
            // Keep the chrome visible during the drag.
            ui.invoke_reveal_chrome_now();
        });
    }

    // Commit on release: jump to the spread containing the released page, then
    // refresh (which re-seeds the scrubber + counter to the committed spread).
    {
        let ui_weak = ui.as_weak();
        let state = Rc::clone(&state);
        let viewport = Rc::clone(&viewport);
        ui.on_scrub_commit(move |page| {
            let Some(ui) = ui_weak.upgrade() else {
                return;
            };
            // borrow_mut() temporary drops at the `;` before refresh borrows state.
            // Refresh unconditionally — unlike the nav handler, a scrub commit always
            // re-seeds the scrubber knob + counter to the committed spread, even when
            // the resolved leading equals the current index (a no-op jump).
            let _moved = state.borrow_mut().jump_to(page.max(0) as usize);
            refresh(&ui, &state.borrow(), &viewport);
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

    // Open the settings dialog: push the current values into the dialog's in-out
    // properties, then show it. Display modes are read from the RUNTIME source of
    // truth (`ViewerState` for direction/spread/cover, `ViewportState` for fit) so
    // the dialog can never show a stale value; cache/preload/track come from
    // `Settings`. `state`, `settings`, and `viewport` are distinct RefCells, so the
    // named borrows `s`/`st` and the temporary `viewport.borrow()` cannot conflict.
    {
        let ui_weak = ui.as_weak();
        let state = Rc::clone(&state);
        let settings = Rc::clone(&settings);
        let viewport = Rc::clone(&viewport);
        ui.on_open_settings(move || {
            let Some(ui) = ui_weak.upgrade() else {
                return;
            };
            let s = settings.borrow();
            let st = state.borrow();
            ui.set_reading_direction_index(reading_direction_to_index(st.reading_direction()));
            ui.set_spread_mode_index(spread_mode_to_index(st.spread_mode()));
            ui.set_cover_mode_index(cover_mode_to_index(st.cover_mode()));
            // Fit mode is owned by the viewport at runtime.
            ui.set_fit_mode_index(fit_mode_to_index(viewport.borrow().fit_mode()));
            ui.set_cache_size(s.cache_size as i32);
            ui.set_preload_pages(s.preload_pages as i32);
            ui.set_track_recent(s.track_recent_files);
            ui.set_key_bindings_text(KEY_BINDINGS_HELP.into());
            ui.set_show_settings(true);
        });
    }

    // Close the settings dialog: hide it, reconcile runtime modes into Settings,
    // persist, then restore focus to the page area so keyboard navigation keeps
    // working. The `reconcile_settings` call's `settings.borrow_mut()` drops at its
    // `;`, so `save()`'s fresh `settings.borrow()` cannot double-borrow.
    {
        let ui_weak = ui.as_weak();
        let state = Rc::clone(&state);
        let settings = Rc::clone(&settings);
        let viewport = Rc::clone(&viewport);
        ui.on_close_settings(move || {
            let Some(ui) = ui_weak.upgrade() else {
                return;
            };
            ui.set_show_settings(false);
            reconcile_settings(
                &state.borrow(),
                &viewport.borrow(),
                &mut settings.borrow_mut(),
            );
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
        let viewport = Rc::clone(&viewport);
        ui.on_set_reading_direction(move |i| {
            let Some(ui) = ui_weak.upgrade() else {
                return;
            };
            let dir = index_to_reading_direction(i);
            // Runtime state is the single source of truth; `reconcile_settings`
            // mirrors it into `Settings` at the next save.
            if state.borrow_mut().set_reading_direction(dir) {
                refresh(&ui, &state.borrow(), &viewport);
            }
        });
    }
    {
        let ui_weak = ui.as_weak();
        let state = Rc::clone(&state);
        let viewport = Rc::clone(&viewport);
        ui.on_set_spread_mode(move |i| {
            let Some(ui) = ui_weak.upgrade() else {
                return;
            };
            let mode = index_to_spread_mode(i);
            // Runtime state is the single source of truth; `reconcile_settings`
            // mirrors it into `Settings` at the next save.
            if state.borrow_mut().set_spread_mode(mode) {
                refresh(&ui, &state.borrow(), &viewport);
            }
        });
    }
    {
        let ui_weak = ui.as_weak();
        let state = Rc::clone(&state);
        let viewport = Rc::clone(&viewport);
        ui.on_set_cover_mode(move |i| {
            let Some(ui) = ui_weak.upgrade() else {
                return;
            };
            let mode = index_to_cover_mode(i);
            // Runtime state is the single source of truth; `reconcile_settings`
            // mirrors it into `Settings` at the next save.
            if state.borrow_mut().set_cover_mode(mode) {
                refresh(&ui, &state.borrow(), &viewport);
            }
        });
    }
    {
        let ui_weak = ui.as_weak();
        let state = Rc::clone(&state);
        let viewport = Rc::clone(&viewport);
        ui.on_set_fit_mode(move |i| {
            let Some(ui) = ui_weak.upgrade() else {
                return;
            };
            let mode = index_to_fit_mode(i);
            // Equality guard (the viewport setter is not idempotent-by-return).
            // Compare in one borrow, mutate in a separate `borrow_mut()` that
            // drops at the `;`, then `refresh` (which borrows viewport internally).
            // The viewport owns `fit_mode` at runtime; `reconcile_settings`
            // mirrors it into `Settings` at the next save.
            if viewport.borrow().fit_mode() != mode {
                viewport.borrow_mut().set_fit(mode);
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
        let viewport = Rc::clone(&viewport);
        let nav = Rc::clone(&nav);
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
                    // Reveal the auto-hiding chrome on a page-turn key (spec §5).
                    ui.invoke_reveal_chrome_now();
                }
                // Runtime state is the single source of truth for these modes;
                // `reconcile_settings` mirrors them into `Settings` at the next
                // save (no per-key Settings write).
                KeyCommand::ToggleSpread => {
                    if state.borrow_mut().toggle_spread() {
                        refresh(&ui, &state.borrow(), &viewport);
                    }
                }
                KeyCommand::ToggleReadingDirection => {
                    if state.borrow_mut().toggle_reading_direction() {
                        refresh(&ui, &state.borrow(), &viewport);
                    }
                }
                KeyCommand::ToggleCover => {
                    if state.borrow_mut().toggle_cover() {
                        refresh(&ui, &state.borrow(), &viewport);
                    }
                }
                // Zoom/fit commands mutate `ViewportState`, then push geometry.
                // The viewport owns `fit_mode` at runtime; `reconcile_settings`
                // mirrors it into `Settings` at the next save (zoom/pan stay
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
                // Fit changes reset zoom + re-center. The viewport owns `fit_mode`;
                // `reconcile_settings` persists it at the next save (zoom/pan are
                // NOT persisted — session-only).
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
                    go_to_library(&ui, &nav);
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
    // Reconcile runtime display modes into Settings, then persist on exit so even a
    // first run writes a file the user can hand-edit. The `reconcile_settings`
    // call's `settings.borrow_mut()` drops at the `;`, so `save()`'s fresh
    // `settings.borrow()` cannot double-borrow. Save failure is logged, not fatal.
    reconcile_settings(
        &state.borrow(),
        &viewport.borrow(),
        &mut settings.borrow_mut(),
    );
    if let Err(e) = settings.borrow().save() {
        tracing::error!(error = %e, "failed to save settings on exit");
    }
    Ok(())
}

/// Open `path`, record it in recent files (when enabled), refresh the view,
/// surface any skipped-entry count, and launch thumbnail generation. Shared by
/// the Open Folder and Open Archive handlers so the "open a book" use-case lives
/// in exactly one place.
///
/// Both directories and archives are opened via `ViewerState::open_folder`, which
/// delegates to `open_path` (dispatching by format through `ArchiveLoader`). Using
/// `open_folder` preserves the directory-open API path and also keeps the method a
/// live runtime caller (avoiding a dead-code warning in a binary crate).
/// `skipped_detail` is appended to the "N entries skipped" status note: `""` for
/// folders, and for archives the archive-specific skip reasons (zip-slip /
/// path-traversal entries and entries exceeding the per-entry size ceiling; see
/// `naming.rs`).
///
/// Thumbnail regeneration is delegated to the shared `ThumbnailController`, which
/// owns the strip's model and the epoch + cancel double-guard; `start` cancels any
/// in-flight generation for the previous book before launching this one.
fn open_and_present(
    ui: &ViewerWindow,
    state: &Rc<RefCell<ViewerState>>,
    settings: &Rc<RefCell<Settings>>,
    viewport: &Rc<RefCell<ViewportState>>,
    thumbs: &ThumbnailController,
    path: &Path,
    skipped_detail: &str, // "" for folders, " (zip-slip or oversized)" for archives
) {
    // Bind the result first so the `state.borrow_mut()` temporary drops before the
    // `Ok` arm reads `state` again (a borrow held across the match would
    // double-borrow-panic at the `reconcile_settings(&state.borrow(), ..)` below).
    let opened = state.borrow_mut().open_folder(path);
    match opened {
        Ok(()) => {
            tracing::info!(path = %path.display(), "opened source");
            let mut s = settings.borrow_mut();
            if s.track_recent_files {
                // Reconcile runtime display modes into Settings just before this
                // open-time save (the only save on this path), reusing the mutable
                // borrow `s`. `state`/`viewport` are distinct RefCells, so their
                // immutable borrows can't conflict with `s` (and the `borrow_mut()`
                // from `open_folder` above already dropped via the `opened` binding).
                reconcile_settings(&state.borrow(), &viewport.borrow(), &mut s);
                s.push_recent(path.to_path_buf());
                if let Err(e) = s.save() {
                    tracing::error!(error = %e, "failed to save settings on open");
                }
            }
        }
        Err(e) => {
            tracing::error!(error = %e, "failed to open source");
            ui.set_status_text(format!("Error: {e}").into());
            return;
        }
    }
    refresh(ui, &state.borrow(), viewport);
    let skipped = state.borrow().last_open_skipped();
    if skipped > 0 {
        let base = ui.get_status_text().to_string();
        ui.set_status_text(
            format!("{base} \u{2014} {skipped} entries skipped{skipped_detail}").into(),
        );
    }
    // Kick off parallel thumbnail generation for the newly opened source.
    thumbs.start(
        ui.as_weak(),
        state.borrow().current_source(),
        state.borrow().page_count(),
    );
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

    // Seed the scrubber + page-counter chrome from the current spread. The
    // counter uses 1-based numbers; `double` mirrors whether the current spread
    // has a trailing page. These are display-only and do NOT change the page body.
    let total = state.page_count();
    let current_1based = if total == 0 { 0 } else { state.index() + 1 };
    ui.set_scrubber_total_pages(total as i32);
    ui.set_scrubber_current_page(current_1based as i32);
    // `preview_is_double` resolves the trailing page using the SAME layout as the
    // body (and is decode-free), so it is the exact "current spread has a trailing
    // page" predicate without re-running `current_spread`'s decode.
    let is_double = state.preview_is_double(state.index());
    ui.set_scrubber_double(is_double);
    // Counter text: "X / N" single, "X\u{2013}Y / N" double, "0 / 0" when empty.
    let counter = if total == 0 {
        "0 / 0".to_string()
    } else if is_double {
        format!(
            "{}\u{2013}{} / {}",
            state.index() + 1,
            state.index() + 2,
            total
        )
    } else {
        format!("{} / {}", state.index() + 1, total)
    };
    ui.set_page_counter_text(counter.into());
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

/// Switch the app to the Library carousel and sync the UI's `screen` property.
/// The single chokepoint for "go to Library" so no caller forgets to sync the
/// UI's `screen` property and restore carousel focus (mirrors `go_to_viewer`).
fn go_to_library(ui: &ViewerWindow, nav: &Rc<RefCell<NavState>>) {
    nav.borrow_mut().to_library();
    ui.set_screen(screen_to_index(nav.borrow().screen()));
    // Restore keyboard focus to the carousel so its key seams work immediately.
    ui.invoke_focus_carousel();
}

/// Switch the app to the Viewer and sync the UI's `screen` property. The single
/// chokepoint for "go to Viewer"; restores focus to the page area so keyboard
/// navigation keeps working.
fn go_to_viewer(ui: &ViewerWindow, nav: &Rc<RefCell<NavState>>) {
    nav.borrow_mut().to_viewer();
    ui.set_screen(screen_to_index(nav.borrow().screen()));
    ui.invoke_focus_pages();
}

/// Convert core RGBA bytes into a `slint::Image`.
fn to_slint_image(decoded: &DecodedImage) -> slint::Image {
    let mut buffer =
        slint::SharedPixelBuffer::<slint::Rgba8Pixel>::new(decoded.width(), decoded.height());
    buffer.make_mut_bytes().copy_from_slice(decoded.rgba());
    slint::Image::from_rgba8(buffer)
}

/// Adapt a pure `CarouselData` row into the Slint `CarouselItem` the carousel
/// renders. Runs on the UI thread (it builds a `slint::Image`). The cover is a
/// PLACEHOLDER (`slint::Image::default()`) this PR; PR-V streams real covers
/// into the same `VecModel<CarouselItem>` via the PR8a `invoke_from_event_loop`
/// pattern, so the carousel shows a neutral placeholder until then.
fn to_carousel_item(data: &CarouselData) -> CarouselItem {
    CarouselItem {
        cover: slint::Image::default(),
        title: data.title.as_str().into(),
        current: data.current,
        total: data.total,
        progress: data.progress,
        available: data.available,
    }
}

/// Build the carousel's backing model from the current `Library`, in shelf
/// order, and bind it to the UI's `carousel-items`. Returns the `Rc<VecModel>`
/// so callers (PR-V cover streaming; PR-L refresh-after-add) can mutate the
/// SAME model rather than rebuilding/rebinding. Centralizes the build + bind so
/// the Library → carousel surface lives in ONE place (mirrors
/// `ThumbnailController::new`).
fn build_carousel_model(ui: &ViewerWindow, library: &Library) -> Rc<VecModel<CarouselItem>> {
    let items: Vec<CarouselItem> = carousel_data(library)
        .iter()
        .map(to_carousel_item)
        .collect();
    let model = Rc::new(VecModel::from(items));
    ui.set_carousel_items(ModelRc::from(model.clone()));
    model
}

/// Fetch the thumbnail image for a 0-based page from the strip's existing
/// `VecModel<ThumbnailItem>` (the PR8a model). Returns the cell's image when the
/// row exists and is loaded; otherwise a default (empty) image. UI-thread only —
/// the `Rc` model is never crossed between threads. No new decode is performed.
fn thumb_image_at(model: &slint::ModelRc<ThumbnailItem>, page: usize) -> slint::Image {
    use slint::Model;
    match model.row_data(page) {
        Some(item) if item.loaded => item.image,
        Some(_) => slint::Image::default(), // still loading: normal, stay silent
        None => {
            // `page` is outside the thumbnail model — strip and page_count are out
            // of sync (the model is built to exactly page_count rows). Not fatal:
            // show a blank preview, but log so the desync is diagnosable.
            tracing::warn!(
                page,
                row_count = model.row_count(),
                "thumb_image_at: page outside thumbnail model (strip/page_count desync)"
            );
            slint::Image::default()
        }
    }
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
  T = toggle thumbnail strip

Library:
  Up = return to the library";

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

/// Copy the runtime-owned display settings into the persisted `Settings` just
/// before saving. This is the SINGLE place `reading_direction`, `spread_mode`,
/// `cover_mode`, and `fit_mode` are written back to `Settings`, so a new
/// mode-mutation site can never "forget to mirror" — it only changes runtime
/// state, and the next save reconciles automatically.
fn reconcile_settings(state: &ViewerState, viewport: &ViewportState, settings: &mut Settings) {
    settings.reading_direction = state.reading_direction();
    settings.spread_mode = state.spread_mode();
    settings.cover_mode = state.cover_mode();
    settings.fit_mode = viewport.fit_mode();
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

    #[test]
    fn reconcile_writes_runtime_modes_into_settings() {
        // Runtime state is the single source of truth: set the three ViewerState
        // modes and the viewport's fit to NON-default values...
        let mut state = ViewerState::new();
        let _ = state.set_reading_direction(ReadingDirection::Rtl);
        let _ = state.set_spread_mode(SpreadMode::Double);
        let _ = state.set_cover_mode(CoverMode::Paired);
        let mut viewport = ViewportState::from_settings(&Settings::default());
        viewport.set_fit(FitMode::Actual);

        // ...start from a Settings whose NON-mirrored fields hold NON-default values
        // (use struct-update syntax to avoid clippy::field_reassign_with_default),
        // so we can prove reconcile touches ONLY the four display-mode fields.
        let mut settings = Settings {
            cache_size: 99,
            preload_pages: 7,
            track_recent_files: true,
            seen_guide: true,
            ..Settings::default()
        };
        reconcile_settings(&state, &viewport, &mut settings);

        // The four mirrored fields now match the runtime (defaults Ltr/Single/
        // Standalone/Whole all DIFFER from the values set above, so this can't pass
        // vacuously)...
        assert_eq!(settings.reading_direction, ReadingDirection::Rtl);
        assert_eq!(settings.spread_mode, SpreadMode::Double);
        assert_eq!(settings.cover_mode, CoverMode::Paired);
        assert_eq!(settings.fit_mode, FitMode::Actual);
        // ...and the unrelated persisted fields are left untouched.
        assert_eq!(settings.cache_size, 99);
        assert_eq!(settings.preload_pages, 7);
        assert!(settings.track_recent_files);
        assert!(settings.seen_guide);
    }
}
