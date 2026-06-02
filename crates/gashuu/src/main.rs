slint::include_modules!();

mod app;
mod carousel;
mod cover_loader;
mod enum_adapters;
mod keymap;
mod library_model;
mod navigation;
mod page_counter;
mod thumbnail_strip;
mod viewer_state;
mod viewport;

use carousel::{build_carousel_model, cover_requests, thumb_image_at};
use enum_adapters::{
    cover_mode_to_index, fit_mode_to_index, index_to_cover_mode, index_to_fit_mode,
    index_to_reading_direction, index_to_spread_mode, reading_direction_to_index,
    spread_mode_to_index,
};
use gashuu_core::{CacheConfig, DecodedImage, FitMode, Library, ReadingDirection, Settings};
use keymap::{map_key, KeyCommand};
use navigation::{screen_to_index, NavState};
use page_counter::page_counter_text;
use std::cell::RefCell;
use std::rc::Rc;
use thumbnail_strip::ThumbnailController;
use viewer_state::{scrub_fraction_to_page, ViewerState};
use viewport::ViewportState;

fn main() -> color_eyre::Result<()> {
    color_eyre::install()?;
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    // Load persisted settings and library; corrupt/unreadable files fall back
    // to defaults (the corrupt-file recovery policy lives here in the UI layer,
    // by design). Missing files return Ok(default) from Settings::load /
    // Library::load, so the Err arm fires only on a GENUINE failure (corrupt
    // data, I/O error, NoDataDir). Errors are collected and surfaced on the
    // home screen after the initial refresh, which itself overwrites
    // status-text, so the notice must be set after that call.
    let mut load_errs: Vec<String> = Vec::new();
    let settings = match Settings::load() {
        Ok(s) => s,
        Err(e) => {
            tracing::warn!(error = %e, "failed to load settings; using defaults");
            load_errs.push(format!("settings ({e})"));
            Settings::default()
        }
    };
    let library = match Library::load() {
        Ok(l) => l,
        Err(e) => {
            tracing::warn!(error = %e, "failed to load library; starting empty");
            load_errs.push(format!("library ({e})"));
            Library::new()
        }
    };

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
    // open handlers (via `OpenBookUseCase`) can share the single controller.
    let thumbs = Rc::new(ThumbnailController::new(&ui));

    // Cover controller for the library carousel. Owns the epoch + cancel
    // double-guard; `start` is called after the carousel model is (re)built so a
    // library refresh supersedes any covers still streaming from the prior view.
    let covers = Rc::new(cover_loader::CoverController::new());

    // The "open a book" use-case, bundling the six collaborators it threads
    // (state, settings, viewport, library, thumbs, covers). Built once and shared
    // via `Rc` by the Open Folder / Open Archive / carousel-open handlers so the
    // open flow lives in exactly one place (`app::OpenBookUseCase`).
    let open_book = Rc::new(app::OpenBookUseCase::new(
        Rc::clone(&state),
        Rc::clone(&settings),
        Rc::clone(&viewport),
        Rc::clone(&library),
        Rc::clone(&thumbs),
        Rc::clone(&covers),
    ));

    // Seed the carousel model from the persisted library so the home screen shows
    // the saved books on boot. The carousel model is bound into the UI inside
    // build_carousel_model; the CoverController re-fetches it through the
    // window's Weak handle inside each event-loop closure, so the returned Rc
    // is not needed here. The `_` prefix permanently suppresses the
    // unused-variable warning.
    let _carousel_model = build_carousel_model(&ui, &library.borrow());

    // Kick off cover loading for the initial library (cache hits paint now; misses
    // stream in from rayon workers).
    covers.start(ui.as_weak(), cover_requests(&library.borrow()));
    ui.set_carousel_focused_index(0);

    // Top-level screen state machine. App boots to Library (the carousel home).
    // Held in an Rc<RefCell<…>> so the carousel callbacks and the Viewer's
    // GoToLibrary key arm can all flip it through the seam functions below.
    let nav = Rc::new(RefCell::new(NavState::new()));
    // Push the initial screen so the window shows the Library on boot.
    ui.set_screen(screen_to_index(nav.borrow().screen()));

    // The centered title-bar name starts blank — nothing is open yet — and is
    // set to the folder/archive name on a successful open (see the open handlers).
    ui.set_current_book_name("".into());

    // Initial paint so rtl/single/status are all initialized before the first
    // folder is opened (refresh shows "No folder opened" and clears the images).
    refresh(&ui, &state.borrow(), &viewport);

    // Surface any load failures AFTER the initial refresh, which overwrites
    // status-text with "No folder opened". The carousel/home screen is visible
    // at this point, so the user sees the notice immediately. Missing files
    // return Ok(default), so this fires only on genuine failures.
    if !load_errs.is_empty() {
        ui.set_status_text(
            format!(
                "Could not load {}; starting fresh.",
                load_errs.join(" and ")
            )
            .into(),
        );
    }

    // First-run guide: show the overlay exactly once. `seen_guide` is flipped and
    // persisted when the user dismisses it (see `on_dismiss_guide`).
    if !settings.borrow().seen_guide {
        ui.set_show_guide(true);
    }

    // Open Folder button: pick a directory, open it, refresh the view, and start thumbnail generation.
    {
        let ui_weak = ui.as_weak();
        let open_book = Rc::clone(&open_book);
        let state = Rc::clone(&state);
        ui.on_open_folder(move || {
            with_ui(&ui_weak, |ui| {
                let Some(dir) = rfd::FileDialog::new().pick_folder() else {
                    return;
                };
                open_book.run(&ui, &dir, "");
                // Title-bar book name is derived from the AUTHORITATIVE post-open
                // state (the canonical `open_file`), not the raw dialog path, so a
                // FAILED open never shows the name of a book that did not open: on
                // failure `open_file` is unchanged (the previously open book, if
                // any) and `run` already set an `Error:` status.
                ui.set_current_book_name(current_book_name(&state).into());
            })
        });
    }

    // Open Archive button: pick a CBZ/ZIP/CBR/RAR file, open it, refresh the view, and start thumbnail generation.
    {
        let ui_weak = ui.as_weak();
        let open_book = Rc::clone(&open_book);
        let state = Rc::clone(&state);
        ui.on_open_archive(move || {
            with_ui(&ui_weak, |ui| {
                let Some(file) = rfd::FileDialog::new()
                    .add_filter("Comic archive", &["cbz", "zip", "cbr", "rar"])
                    .pick_file()
                else {
                    return;
                };
                open_book.run(&ui, &file, " (zip-slip or oversized)");
                // Title-bar book name is derived from the AUTHORITATIVE post-open
                // state (the canonical `open_file`), so a FAILED open (corrupt /
                // non-archive file) never shows the picked file's name: on failure
                // `open_file` is unchanged and `run` already set an `Error:` status.
                ui.set_current_book_name(current_book_name(&state).into());
            })
        });
    }

    // Add Files button: pick one or more comic-archive files and add them to the
    // library. Skips duplicates (via `add_paths`), persists, rebuilds the carousel
    // model, and restores keyboard focus to the carousel.
    {
        let ui_weak = ui.as_weak();
        let library = Rc::clone(&library);
        let covers = Rc::clone(&covers);
        ui.on_add_files(move || {
            with_ui(&ui_weak, |ui| {
                let Some(files) = rfd::FileDialog::new()
                    .add_filter("Comic archive", &["cbz", "zip", "cbr", "rar"])
                    .pick_files()
                else {
                    return;
                };
                add_books_and_refresh(&ui, &library, &covers, files, "add-files");
            })
        });
    }

    // Add Folder button: pick a single folder and add it as one book to the
    // library. Wraps the folder in a `vec![]` so the same dedup/save/rebuild
    // path as `on_add_files` is used. Persists and restores carousel focus.
    {
        let ui_weak = ui.as_weak();
        let library = Rc::clone(&library);
        let covers = Rc::clone(&covers);
        ui.on_add_folder(move || {
            with_ui(&ui_weak, |ui| {
                let Some(folder) = rfd::FileDialog::new().pick_folder() else {
                    return;
                };
                add_books_and_refresh(&ui, &library, &covers, vec![folder], "add-folder");
            })
        });
    }

    // Carousel: Return on the focused book opens it, resumes its last-read
    // page (via OpenBookUseCase::run → jump_to), and transitions to the Viewer.
    {
        let ui_weak = ui.as_weak();
        let library = Rc::clone(&library);
        let nav = Rc::clone(&nav);
        let open_book = Rc::clone(&open_book);
        let state = Rc::clone(&state);
        ui.on_carousel_open(move |index| {
            with_ui(&ui_weak, |ui| {
                // Resolve the focused carousel index to a Library book path.
                // Borrow discipline: `library.borrow()` drops at the `;`.
                let path = {
                    let lib = library.borrow();
                    lib.books()
                        .get(index as usize)
                        .map(|b| b.path().to_path_buf())
                };
                let Some(path) = path else {
                    // Index out of range (carousel and library out of sync) — no-op.
                    tracing::warn!(index, "carousel-open: no book at index");
                    return;
                };
                // open_book.run writes back the OLD book's position first,
                // then opens the new path and resumes its stored position.
                open_book.run(&ui, &path, "");
                // Title-bar book name is derived from the AUTHORITATIVE post-open
                // state (the canonical `open_file`), so a FAILED open (a Library
                // book that was moved/deleted) never shows that book's name: on
                // failure `open_file` is unchanged and `run` already set an
                // `Error:` status. The pre-existing `go_to_viewer` navigation is
                // intentionally left unchanged (out of scope here).
                ui.set_current_book_name(current_book_name(&state).into());
                go_to_viewer(&ui, &nav);
            })
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
            with_ui(&ui_weak, |ui| {
                let count = library.borrow().books().len();
                if count == 0 {
                    return; // empty shelf: nothing to move
                }
                let last = (count - 1) as i32;
                let next = (ui.get_carousel_focused_index() + delta).clamp(0, last);
                ui.set_carousel_focused_index(next);
            })
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
            with_ui(&ui_weak, |ui| {
                go_to_viewer(&ui, &nav);
            })
        });
    }

    // Thumbnail click: jump to the clicked page's spread, refresh, then restore
    // focus to the page area so keyboard navigation keeps working.
    {
        let ui_weak = ui.as_weak();
        let state = Rc::clone(&state);
        let viewport = Rc::clone(&viewport);
        ui.on_thumbnail_clicked(move |page| {
            with_ui(&ui_weak, |ui| {
                if state.borrow_mut().jump_to(page as usize) {
                    refresh(&ui, &state.borrow(), &viewport);
                }
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
                // Pull thumbnail images from the existing model (no decode).
                let model = ui.get_thumbnails();
                ui.set_scrubber_preview_a(thumb_image_at(&model, lead));
                ui.set_scrubber_preview_b(match trail {
                    Some(trail) => thumb_image_at(&model, trail),
                    None => slint::Image::default(),
                });
                // Update the counter to the previewed page (1-based).
                let counter = page_counter_text(lead, trail, total);
                ui.set_page_counter_text(counter.into());
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
                refresh(&ui, &state.borrow(), &viewport);
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
            with_ui(&ui_weak, |ui| {
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
            })
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
            with_ui(&ui_weak, |ui| {
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
            })
        });
    }

    // Dismiss the first-run guide: mark it seen, persist, hide it, restore focus.
    // Two-statement RefCell discipline: the `borrow_mut()` drops at the `;` before
    // the immutable `borrow()` for `save`.
    {
        let ui_weak = ui.as_weak();
        let settings = Rc::clone(&settings);
        ui.on_dismiss_guide(move || {
            with_ui(&ui_weak, |ui| {
                // Persist immediately; a persistent save failure here is non-fatal — the
                // guide simply re-shows next launch (seen_guide is also saved on exit).
                settings.borrow_mut().seen_guide = true;
                if let Err(e) = settings.borrow().save() {
                    tracing::error!(error = %e, "failed to save settings on guide dismiss");
                }
                ui.set_show_guide(false);
                ui.invoke_focus_pages();
            })
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
            with_ui(&ui_weak, |ui| {
                let dir = index_to_reading_direction(i);
                // Runtime state is the single source of truth; `reconcile_settings`
                // mirrors it into `Settings` at the next save.
                if state.borrow_mut().set_reading_direction(dir) {
                    refresh(&ui, &state.borrow(), &viewport);
                }
            })
        });
    }
    {
        let ui_weak = ui.as_weak();
        let state = Rc::clone(&state);
        let viewport = Rc::clone(&viewport);
        ui.on_set_spread_mode(move |i| {
            with_ui(&ui_weak, |ui| {
                let mode = index_to_spread_mode(i);
                // Runtime state is the single source of truth; `reconcile_settings`
                // mirrors it into `Settings` at the next save.
                if state.borrow_mut().set_spread_mode(mode) {
                    refresh(&ui, &state.borrow(), &viewport);
                }
            })
        });
    }
    {
        let ui_weak = ui.as_weak();
        let state = Rc::clone(&state);
        let viewport = Rc::clone(&viewport);
        ui.on_set_cover_mode(move |i| {
            with_ui(&ui_weak, |ui| {
                let mode = index_to_cover_mode(i);
                // Runtime state is the single source of truth; `reconcile_settings`
                // mirrors it into `Settings` at the next save.
                if state.borrow_mut().set_cover_mode(mode) {
                    refresh(&ui, &state.borrow(), &viewport);
                }
            })
        });
    }
    {
        let ui_weak = ui.as_weak();
        let state = Rc::clone(&state);
        let viewport = Rc::clone(&viewport);
        ui.on_set_fit_mode(move |i| {
            with_ui(&ui_weak, |ui| {
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
            })
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
            state
                .borrow_mut()
                .set_cache_config(CacheConfig::new(v, preload));
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
            state
                .borrow_mut()
                .set_cache_config(CacheConfig::new(cache_size, v));
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

    // Keyboard navigation forwarded from the FocusScope.
    {
        let ui_weak = ui.as_weak();
        let state = Rc::clone(&state);
        let viewport = Rc::clone(&viewport);
        let nav = Rc::clone(&nav);
        let library = Rc::clone(&library);
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
                        // Write the current position back before leaving the viewer.
                        // Borrow discipline: write_back_position takes &Rc<RefCell<…>>
                        // and confines each borrow to a single statement; drops before
                        // go_to_library borrows the UI.
                        write_back_position(&state, &library);
                        go_to_library(&ui, &nav);
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
        ui.on_resized(move |w, h| {
            with_ui(&ui_weak, |ui| {
                if state.borrow_mut().set_viewport_size(w, h) {
                    refresh(&ui, &state.borrow(), &viewport);
                }
            })
        });
    }

    ui.run()?;
    // Write the current reading position back to the library before exit.
    // The `state` and `library` RefCells are no longer borrowed (the event
    // loop has exited), so there is no borrow conflict here.
    write_back_position(&state, &library);
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

/// Upgrade a window `Weak` and run `f` with the live `ViewerWindow`, or no-op if
/// the window is gone (teardown race). Replaces the repeated
/// `let Some(ui) = ui_weak.upgrade() else { return; };` preamble.
fn with_ui(weak: &slint::Weak<ViewerWindow>, f: impl FnOnce(ViewerWindow)) {
    if let Some(ui) = weak.upgrade() {
        f(ui);
    }
}

/// Push the current spread + status into the UI, then re-anchor the viewport to
/// the new content size and push the resulting geometry.
pub(crate) fn refresh(
    ui: &ViewerWindow,
    state: &ViewerState,
    viewport: &Rc<RefCell<ViewportState>>,
) {
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
    let trailing = if is_double {
        Some(state.index() + 1)
    } else {
        None
    };
    let counter = page_counter_text(state.index(), trailing, total);
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

/// Copy the runtime-owned display settings into the persisted `Settings` just
/// before saving. This is the SINGLE place `reading_direction`, `spread_mode`,
/// `cover_mode`, and `fit_mode` are written back to `Settings`, so a new
/// mode-mutation site can never "forget to mirror" — it only changes runtime
/// state, and the next save reconciles automatically.
pub(crate) fn reconcile_settings(
    state: &ViewerState,
    viewport: &ViewportState,
    settings: &mut Settings,
) {
    settings.reading_direction = state.reading_direction();
    settings.spread_mode = state.spread_mode();
    settings.cover_mode = state.cover_mode();
    settings.fit_mode = viewport.fit_mode();
}

/// Derive the centered title-bar display name from the AUTHORITATIVE post-open
/// state, so it can never show a book that did not actually open.
///
/// Reads the canonical `open_file()` from `ViewerState` (the same key the
/// library write-back uses), which is `Some(path)` after a successful open of a
/// folder OR an archive and is left UNCHANGED on a failed open (`open_path`
/// returns early via `?` before `set_source`). Therefore:
///   - success  -> the just-opened book's name (from the canonical path);
///   - failure with a prior book still open -> that book's name (still shown);
///   - failure with nothing open / boot -> `""`.
///
/// `open_file` is a real filesystem path, so `is_dir()` reliably discriminates a
/// folder from an archive for the name component. Borrow discipline: the single
/// `state.borrow()` `Ref` is confined to this function and drops on return.
fn current_book_name(state: &Rc<RefCell<ViewerState>>) -> String {
    let s = state.borrow();
    match s.open_file() {
        Some(path) => book_name_for(path, path.is_dir()),
        None => String::new(),
    }
}

/// Derive a book's display name from its path, mirroring the `Book::from_path`
/// convention: a folder shows its directory name (last path component), an
/// archive shows its file stem (extension dropped so `.cbz`/`.zip` don't clutter
/// the title). Falls back to the lossy full path so the name is never empty.
/// `is_dir` is the folder/archive discriminator. `to_string_lossy` never panics
/// on a non-UTF-8 path.
fn book_name_for(path: &std::path::Path, is_dir: bool) -> String {
    let component = if is_dir {
        path.file_name()
    } else {
        path.file_stem()
    };
    component
        .map(|s| s.to_string_lossy().into_owned())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| path.to_string_lossy().into_owned())
}

/// Pure helper: decide if and what to write back to the Library.
///
/// Returns `Some((canonical_path, page_index))` when a write-back should be
/// performed (a book is open), `None` otherwise. Extracted for table-testing
/// so the predicate can be verified independently of the effectful
/// `write_back_position` that actually calls `library.set_last_page`.
fn position_to_write_back(
    open_file: Option<&std::path::Path>,
    page: usize,
) -> Option<(std::path::PathBuf, usize)> {
    open_file.map(|p| (p.to_path_buf(), page))
}

/// Write the current reading position back to the Library and persist.
///
/// Called at every leave point: ↑ to Library, opening a different book,
/// and app exit. `set_last_page` returns `false` when the path is absent or
/// the value is unchanged (idempotent). We do not guard `save()` on that
/// return value — we always persist for simplicity (one short JSON write at
/// most, and the result is idempotent on disk).
///
/// Borrow discipline: `state` and `library` are distinct `RefCell`s, so
/// borrowing one never affects the other. The opening `let` takes a single
/// shared borrow of `state` and reads both fields from it; that `Ref` drops at
/// the end of the statement, before `library` is borrowed. Each statement's
/// borrows drop before the next statement acquires a different borrow,
/// following the one-statement rule in `docs/patterns.md`.
pub(crate) fn write_back_position(
    state: &Rc<RefCell<ViewerState>>,
    library: &Rc<RefCell<Library>>,
) {
    // Extract the (path, page) tuple from the viewer state under one shared
    // borrow; the `Ref` drops at the `;` before `library` is borrowed.
    let Some((path, page)) = ({
        let s = state.borrow();
        position_to_write_back(s.open_file(), s.index())
    }) else {
        return; // no book open — nothing to write back
    };
    // `set_last_page` returns false when absent or unchanged; we persist
    // unconditionally for simplicity (short JSON write, idempotent on disk).
    library.borrow_mut().set_last_page(&path, page);
    if let Err(e) = library.borrow().save() {
        tracing::error!(error = %e, "failed to save library on position write-back");
    }
}

/// Add every path in `paths` to `lib`, skipping duplicates.
/// Returns the count of books actually inserted (new books only).
/// Dedup is handled by `Library::add` (returns `false` when already present),
/// covering both books already in the library and duplicates within `paths`.
fn add_paths(lib: &mut Library, paths: Vec<std::path::PathBuf>) -> usize {
    paths.into_iter().filter(|p| lib.add(p.clone())).count()
}

/// Add `paths` to the library, persist, rebuild the carousel, and surface the
/// outcome on the status line, restoring carousel focus in every case.
///
/// Shared by the Add Files and Add Folder handlers; `op` distinguishes the two
/// only in the save-failure trace message. When nothing new is added there is
/// nothing to persist or rebuild, so it short-circuits after the status update.
fn add_books_and_refresh(
    ui: &ViewerWindow,
    library: &Rc<RefCell<Library>>,
    covers: &cover_loader::CoverController,
    paths: Vec<std::path::PathBuf>,
    op: &'static str,
) {
    let added = add_paths(&mut library.borrow_mut(), paths);
    if added == 0 {
        // Everything picked was already in the library: nothing to persist or rebuild.
        ui.set_status_text("Already in library \u{2014} no new books added.".into());
        ui.invoke_focus_carousel();
        return;
    }
    // Rebuild from the in-memory state even if the save fails, so the newly added
    // books are visible; the save error is then surfaced (not just traced).
    // `build_carousel_model` rebuilds AND rebinds the model into the UI internally
    // (the returned `Rc<VecModel>` is held by PR-V for cover streaming; the add
    // path does not need it, so the return value is dropped).
    let save_result = library.borrow().save();
    build_carousel_model(ui, &library.borrow());
    // Refresh covers for the new library state; the epoch bump cancels any covers
    // still streaming from the pre-refresh view.
    covers.start(ui.as_weak(), cover_requests(&library.borrow()));
    match save_result {
        Err(e) => {
            tracing::error!(error = %e, "failed to save library after {op}");
            ui.set_status_text(
                format!("Added {added} book(s), but could not save library: {e}").into(),
            );
        }
        Ok(()) => {
            ui.set_status_text(format!("Added {added} book(s)").into());
        }
    }
    ui.invoke_focus_carousel();
}

#[cfg(test)]
mod tests {
    use super::*;
    use gashuu_core::{CoverMode, SpreadMode};

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

    // ---- current_book_name (#71 title-bar) -------------------------------

    #[test]
    fn current_book_name_empty_after_failed_open() {
        // The bug guard: a FAILED open must leave the title-bar name empty when
        // nothing was previously open. `current_book_name` reads the authoritative
        // `open_file()` (left None by a failed `open_folder`), not the dialog path,
        // so it can never surface the name of a book that did not open.
        let state = Rc::new(RefCell::new(ViewerState::new()));
        // Sanity: blank before any open.
        assert_eq!(current_book_name(&state), "");
        // A nonexistent path makes `open_folder` return Err before `set_source`,
        // so `open_file()` stays None and the derived name stays empty.
        let _ = state
            .borrow_mut()
            .open_folder(std::path::Path::new("/nonexistent_gashuu_title_guard"));
        assert_eq!(
            current_book_name(&state),
            "",
            "a failed open must not set a title-bar name"
        );
    }

    #[test]
    fn current_book_name_is_folder_name_after_successful_open() {
        // A SUCCESSFUL folder open derives the directory name from the canonical
        // `open_file()`. Uses a real temp dir (an empty folder opens fine as a
        // FolderSource), mirroring `viewer_state::tests` for open_file.
        let dir = std::env::temp_dir().join(format!("gashuu_title_ok_{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("create temp dir");
        let leaf = dir
            .file_name()
            .expect("temp dir has a name")
            .to_string_lossy()
            .into_owned();

        let state = Rc::new(RefCell::new(ViewerState::new()));
        state
            .borrow_mut()
            .open_folder(&dir)
            .expect("open_folder on a real directory must succeed");
        assert_eq!(
            current_book_name(&state),
            leaf,
            "a successful folder open shows the folder's directory name"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    // ---- position_to_write_back (PR-R) ------------------------------------

    #[test]
    fn position_to_write_back_none_when_no_open_file() {
        assert!(
            position_to_write_back(None, 5).is_none(),
            "no open file => no write-back"
        );
    }

    #[test]
    fn position_to_write_back_some_when_file_open() {
        use std::path::PathBuf;
        let path = PathBuf::from("/some/book.cbz");
        let result = position_to_write_back(Some(path.as_path()), 7);
        assert!(result.is_some(), "open file => write-back tuple");
        let (p, pg) = result.unwrap();
        assert_eq!(p, path);
        assert_eq!(pg, 7);
    }

    #[test]
    fn position_to_write_back_zero_page() {
        use std::path::PathBuf;
        let path = PathBuf::from("/some/book.cbz");
        let result = position_to_write_back(Some(path.as_path()), 0);
        assert!(result.is_some());
        let (_, pg) = result.unwrap();
        assert_eq!(pg, 0, "page 0 is a valid write-back (start of book)");
    }

    #[test]
    fn add_paths_empty_vec_returns_zero() {
        let mut lib = gashuu_core::Library::new();
        let added = add_paths(&mut lib, vec![]);
        assert_eq!(added, 0);
        assert_eq!(lib.books().len(), 0);
    }

    #[test]
    fn add_paths_new_paths_counted() {
        let mut lib = gashuu_core::Library::new();
        let paths = vec![
            std::path::PathBuf::from("nonexistent/vol1.cbz"),
            std::path::PathBuf::from("nonexistent/vol2.cbz"),
        ];
        let added = add_paths(&mut lib, paths);
        assert_eq!(added, 2);
        assert_eq!(lib.books().len(), 2);
    }

    #[test]
    fn add_paths_dedup_within_batch() {
        let mut lib = gashuu_core::Library::new();
        let paths = vec![
            std::path::PathBuf::from("nonexistent/vol1.cbz"),
            std::path::PathBuf::from("nonexistent/vol1.cbz"),
        ];
        let added = add_paths(&mut lib, paths);
        assert_eq!(
            added, 1,
            "duplicate within the batch must not be double-counted"
        );
        assert_eq!(lib.books().len(), 1);
    }

    #[test]
    fn add_paths_dedup_against_existing() {
        let mut lib = gashuu_core::Library::new();
        lib.add(std::path::PathBuf::from("nonexistent/vol1.cbz"));
        let paths = vec![
            std::path::PathBuf::from("nonexistent/vol1.cbz"),
            std::path::PathBuf::from("nonexistent/vol2.cbz"),
        ];
        let added = add_paths(&mut lib, paths);
        assert_eq!(
            added, 1,
            "a path already in the library must not be counted"
        );
        assert_eq!(lib.books().len(), 2);
    }

    #[test]
    fn add_paths_preserves_insertion_order() {
        let mut lib = gashuu_core::Library::new();
        let paths: Vec<_> = (0..5)
            .map(|i| std::path::PathBuf::from(format!("nonexistent/vol{i}.cbz")))
            .collect();
        let added = add_paths(&mut lib, paths);
        assert_eq!(added, 5);
        let titles: Vec<&str> = lib.books().iter().map(|b| b.title()).collect();
        assert_eq!(titles, ["vol0", "vol1", "vol2", "vol3", "vol4"]);
    }

    #[test]
    fn add_paths_all_existing_returns_zero() {
        let mut lib = gashuu_core::Library::new();
        lib.add(std::path::PathBuf::from("nonexistent/vol1.cbz"));
        lib.add(std::path::PathBuf::from("nonexistent/vol2.cbz"));
        let before = lib.books().len();
        let added = add_paths(
            &mut lib,
            vec![
                std::path::PathBuf::from("nonexistent/vol1.cbz"),
                std::path::PathBuf::from("nonexistent/vol2.cbz"),
            ],
        );
        assert_eq!(added, 0, "all-duplicate batch must return 0");
        assert_eq!(lib.books().len(), before, "books count must not change");
    }

    // Note: `build_carousel_model` now takes a `&ViewerWindow` (it builds AND
    // binds the Slint model), so it cannot be unit-tested headless. The
    // Library -> carousel row mapping invariants (length, 1-based `current`,
    // availability, insertion order) are covered by `library_model::tests`
    // against the pure `carousel_data` helper that this builder delegates to.
}
