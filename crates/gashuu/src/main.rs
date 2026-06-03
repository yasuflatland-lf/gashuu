slint::include_modules!();

mod app;
mod carousel;
mod cover_loader;
mod enum_adapters;
mod keymap;
mod library_model;
mod navigation;
mod page_jump;
mod thumbnail_strip;
mod viewer_state;
mod viewport;

use carousel::{
    bind_carousel_model, build_carousel_model, cover_requests, thumb_state_at, ThumbState,
};
use enum_adapters::{
    cover_mode_to_index, fit_mode_to_index, index_to_cover_mode, index_to_fit_mode,
    index_to_reading_direction, index_to_spread_mode, reading_direction_to_index,
    spread_mode_to_index,
};
use gashuu_core::{
    CacheConfig, DecodedImage, FitMode, Library, ReadingDirection, Settings, ViewOverride,
};
use keymap::{map_key, KeyCommand};
use library_model::LibrarySearchState;
use navigation::{screen_to_index, NavState};
use page_jump::parse_page_jump;
use std::cell::RefCell;
use std::rc::Rc;
use thumbnail_strip::ThumbnailController;
use viewer_state::{scrub_fraction_to_page, ViewerState};
use viewport::ViewportState;

fn main() -> color_eyre::Result<()> {
    color_eyre::install()?;
    // Slint's text layout (parley -> icu_segmenter) emits a `log::warn!` for every
    // CJK run because ICU4X bundles no Japanese line-break dictionary. Segmentation
    // still works via a per-character fallback, so the resulting "ICU4X data error:
    // No segmentation model for language: ja" lines are pure noise. They reach this
    // subscriber through tracing-subscriber's tracing-log bridge; silence the
    // `icu_provider` target that emits them while leaving any RUST_LOG override for
    // our own targets intact (a target directive overrides the global default).
    let env_filter = tracing_subscriber::EnvFilter::from_default_env().add_directive(
        "icu_provider=off"
            .parse()
            .expect("static directive is valid"),
    );
    tracing_subscriber::fmt().with_env_filter(env_filter).init();

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

    // Shared library-search filter state. Owned here and shared via `Rc` with the
    // search-query callback (live filtering), the add/open backfill paths, and the
    // open-time page-count rebuild in `OpenBookUseCase`, so every path projects the
    // SAME visible-index set. Starts on the empty query (every book visible).
    let search = Rc::new(RefCell::new(LibrarySearchState::default()));
    ui.set_library_search_query("".into());
    // Seed the visible set against the loaded library under the empty query
    // (every book visible). `set_query` recomputes internally, so the search
    // state is consistent before the first `refresh_library_carousel`, which now
    // only READS `visible_indices()`.
    search
        .borrow_mut()
        .set_query(String::new(), &library.borrow());

    // The "open a book" use-case, bundling the shared collaborators it threads
    // (state, settings, viewport, library, thumbs, covers, search). Built once and
    // shared via `Rc` by the Open Folder / Open Archive / carousel-open handlers so
    // the open flow lives in exactly one place (`app::OpenBookUseCase`). The search
    // state lets the open-time page-count rebuild preserve the active filter.
    let open_book = Rc::new(app::OpenBookUseCase::new(
        Rc::clone(&state),
        Rc::clone(&settings),
        Rc::clone(&viewport),
        Rc::clone(&library),
        Rc::clone(&thumbs),
        Rc::clone(&covers),
        Rc::clone(&search),
    ));

    // Seed the carousel from the persisted library so the home screen shows the
    // saved books on boot. The empty-query visible set was already computed by the
    // `set_query(String::new(), …)` seed above; `refresh_library_carousel` reads
    // those visible indices, builds + binds the filtered model, resets carousel
    // focus to 0, and starts cover loading for the visible rows in ONE place — so
    // the initial build and every later filter/add refresh share the same code
    // path. Cover streaming is started exactly once here (no separate
    // `covers.start`).
    refresh_library_carousel(&ui, &library, &covers, &search, true);

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
        let search = Rc::clone(&search);
        ui.on_add_files(move || {
            with_ui(&ui_weak, |ui| {
                let Some(files) = rfd::FileDialog::new()
                    .add_filter("Comic archive", &["cbz", "zip", "cbr", "rar"])
                    .pick_files()
                else {
                    return;
                };
                add_books_and_refresh(&ui, &library, &covers, &search, files, "add-files");
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
        let search = Rc::clone(&search);
        ui.on_add_folder(move || {
            with_ui(&ui_weak, |ui| {
                let Some(folder) = rfd::FileDialog::new().pick_folder() else {
                    return;
                };
                add_books_and_refresh(&ui, &library, &covers, &search, vec![folder], "add-folder");
            })
        });
    }

    // Library search: the debounced query from the NavBar search field. This is
    // the ONLY query-update path from Slint. Replace the filter (clearing any
    // forced-visible just-added books), then rebuild the filtered carousel + cover
    // stream and reset focus to row 0 so the first match is centered.
    {
        let ui_weak = ui.as_weak();
        let library = Rc::clone(&library);
        let covers = Rc::clone(&covers);
        let search = Rc::clone(&search);
        ui.on_library_search_changed(move |query| {
            with_ui(&ui_weak, |ui| {
                // `search` and `library` are distinct RefCells; borrowing one
                // mut while the other is shared cannot conflict. The shared
                // `library.borrow()` drops at the `;` before refresh.
                search
                    .borrow_mut()
                    .set_query(query.to_string(), &library.borrow());
                refresh_library_carousel(&ui, &library, &covers, &search, true);
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
        let search = Rc::clone(&search);
        ui.on_carousel_open(move |index| {
            with_ui(&ui_weak, |ui| {
                // Resolve the focused VISIBLE carousel index to a Library book
                // path through the search state's projection: the carousel row is
                // an index into `visible_indices`, which maps to the underlying
                // library row. Capture the visible/library lengths INSIDE the
                // borrow scope so the out-of-range warn below can report them
                // without a fresh borrow. Both `Ref`s drop at the block's `}`.
                let (path, visible_len, library_len) = {
                    let search = search.borrow();
                    let visible = search.visible_indices().get(index as usize).copied();
                    let lib = library.borrow();
                    let path = visible
                        .and_then(|library_index| lib.books().get(library_index))
                        .map(|book| book.path().to_path_buf());
                    (path, search.visible_indices().len(), lib.books().len())
                };
                let Some(path) = path else {
                    // Index out of range (carousel and library out of sync) — no-op.
                    tracing::warn!(
                        index,
                        visible_len,
                        library_len,
                        "carousel-open: no book at index"
                    );
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
        let search = Rc::clone(&search);
        ui.on_carousel_move(move |delta| {
            with_ui(&ui_weak, |ui| {
                // Clamp to the VISIBLE (filtered) row count, not the full library:
                // Left/Right move within the currently displayed carousel slice.
                let count = search.borrow().visible_indices().len();
                if count == 0 {
                    return; // empty shelf or no matches: nothing to move
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

    // On invalid or no-op input the field snaps back to the current page; this
    // is the feedback mechanism instead of a visible error message.
    {
        let ui_weak = ui.as_weak();
        let state = Rc::clone(&state);
        let viewport = Rc::clone(&viewport);
        ui.on_page_jump_request(move |text: slint::SharedString| {
            with_ui(&ui_weak, |ui| {
                let total = state.borrow().page_count();
                let did_jump = if let Some(page_0based) = parse_page_jump(text.as_str(), total) {
                    let moved = state.borrow_mut().jump_to(page_0based);
                    if moved {
                        refresh(&ui, &state.borrow(), &viewport);
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
                // screen 1 = Viewer (per-book), screen 0 = Library (global defaults).
                let per_book = ui.get_screen() == 1;
                // On the Library screen the dialog edits the GLOBAL defaults, so
                // mirror them into the runtime first (the dialog seeds from the
                // runtime). The viewer isn't shown on the Library screen, so this
                // has no visible effect; a book re-applies its own override on open.
                if !per_book {
                    apply_global_view_to_runtime(&settings, &state, &viewport);
                }
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
                ui.set_settings_per_book(per_book);
                ui.set_show_settings(true);
            })
        });
    }

    // Close the settings dialog: hide it, reconcile runtime modes into Settings,
    // persist, then restore keyboard focus to whichever screen is underneath.
    // The dialog can be opened from EITHER the Viewer title bar (screen 1) or the
    // Library glass-pill nav (screen 0), so focus must return to the matching
    // FocusScope: the page area on the Viewer, the carousel on the Library.
    // Restoring `focus-pages()` unconditionally would focus the hidden Viewer
    // scope when closing over the Library, leaving the carousel keys dead.
    // All three temporaries (state.borrow(), viewport.borrow(), settings.borrow_mut())
    // are argument expressions in the single reconcile_settings(...) call and drop
    // together at its `;`, before save()'s fresh settings.borrow() on the next line.
    {
        let ui_weak = ui.as_weak();
        let state = Rc::clone(&state);
        let settings = Rc::clone(&settings);
        let viewport = Rc::clone(&viewport);
        let library = Rc::clone(&library);
        ui.on_close_settings(move || {
            with_ui(&ui_weak, |ui| {
                ui.set_show_settings(false);
                // screen 0 = Library (edits GLOBAL defaults), 1 = Viewer (edits the
                // CURRENT book's per-book override).
                if ui.get_screen() == 0 {
                    reconcile_settings(
                        &state.borrow(),
                        &viewport.borrow(),
                        &mut settings.borrow_mut(),
                    );
                    if let Err(e) = settings.borrow().save() {
                        tracing::error!(error = %e, "failed to save settings from dialog");
                        ui.set_status_text(format!("Could not save settings: {e}").into());
                    }
                    ui.invoke_focus_carousel();
                } else {
                    // Persist the four view modes to this book's override. The
                    // cache/preload/track fields are global; save Settings too so a
                    // change to them in the viewer dialog is not lost (the view-mode
                    // fields in Settings are untouched because we did NOT reconcile).
                    write_back_view_override(&state, &viewport, &library);
                    if let Err(e) = settings.borrow().save() {
                        tracing::error!(error = %e, "failed to save settings from dialog");
                        ui.set_status_text(format!("Could not save settings: {e}").into());
                    }
                    ui.invoke_focus_pages();
                }
            })
        });
    }

    // Reset-to-global (viewer settings only): clear THIS book's override so it
    // inherits the global defaults again, apply them to the live view, and re-seed
    // the open dialog's combos to the now-global values.
    {
        let ui_weak = ui.as_weak();
        let state = Rc::clone(&state);
        let settings = Rc::clone(&settings);
        let viewport = Rc::clone(&viewport);
        let library = Rc::clone(&library);
        ui.on_reset_overrides(move || {
            with_ui(&ui_weak, |ui| {
                if let Some(path) = state.borrow().open_file().map(|p| p.to_path_buf()) {
                    let changed = library.borrow_mut().set_overrides(&path, ViewOverride::none());
                    if !changed {
                        tracing::warn!(path = %path.display(), "reset override: open book not found in library");
                    }
                    if let Err(e) = library.borrow().save() {
                        tracing::error!(error = %e, "failed to save library on override reset");
                        ui.set_status_text(format!("Could not save settings: {e}").into());
                    }
                }
                // Apply the global defaults to the runtime + view.
                apply_global_view_to_runtime(&settings, &state, &viewport);
                refresh(&ui, &state.borrow(), &viewport);
                // Sync the open dialog's combos to the now-global values.
                let st = state.borrow();
                ui.set_reading_direction_index(reading_direction_to_index(st.reading_direction()));
                ui.set_spread_mode_index(spread_mode_to_index(st.spread_mode()));
                ui.set_cover_mode_index(cover_mode_to_index(st.cover_mode()));
                ui.set_fit_mode_index(fit_mode_to_index(viewport.borrow().fit_mode()));
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
                // Mutates the runtime view mode only; while a book is open this change
                // is persisted to the current book's per-book override via
                // `write_back_view_override` at the next viewer leave point, not into
                // the global `Settings`.
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
                // Mutates the runtime view mode only; while a book is open this change
                // is persisted to the current book's per-book override via
                // `write_back_view_override` at the next viewer leave point, not into
                // the global `Settings`.
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
                // Mutates the runtime view mode only; while a book is open this change
                // is persisted to the current book's per-book override via
                // `write_back_view_override` at the next viewer leave point, not into
                // the global `Settings`.
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
                // The viewport owns `fit_mode` at runtime; while a book is open this
                // change is persisted to the current book's per-book override via
                // `write_back_view_override` at the next viewer leave point, not into
                // the global `Settings`.
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
                        // Write the current position AND this book's view modes back
                        // before leaving the viewer, so a D/R/C/fit toggle made while
                        // reading persists to the book even without opening settings.
                        // Each helper confines its borrows to single statements; both
                        // drop before go_to_library borrows the UI.
                        write_back_position(&state, &library);
                        write_back_view_override(&state, &viewport, &library);
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
    // Persist any page counts the cover prefetch resolved after the last carousel
    // refresh, so a book counted this session shows its real total next launch
    // instead of being re-counted by re-opening its archive. Safe here: the event
    // loop has exited, so `library` is unborrowed.
    covers.flush_counts(&library);
    // Write the current reading position back to the library before exit.
    // The `state` and `library` RefCells are no longer borrowed (the event
    // loop has exited), so there is no borrow conflict here.
    write_back_position(&state, &library);
    // Persist the open book's view modes to its override on exit (no-op if no book
    // is open), so a toggle made right before quitting is not lost.
    write_back_view_override(&state, &viewport, &library);
    // Only mirror runtime display modes into the GLOBAL Settings when NO book is
    // open — otherwise the open book's per-book modes would clobber the global
    // defaults (its modes were just saved to its override above). cache/preload/
    // track and seen_guide are saved unconditionally below.
    if state.borrow().open_file().is_none() {
        reconcile_settings(
            &state.borrow(),
            &viewport.borrow(),
            &mut settings.borrow_mut(),
        );
    }
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

    // Seed the scrubber chrome from the current spread. The scrubber uses 1-based
    // numbers; `double` mirrors whether the current spread has a trailing page.
    // These are display-only and do NOT change the page body.
    let total = state.page_count();
    let current_1based = current_page_1based(state);
    ui.set_scrubber_total_pages(total as i32);
    ui.set_scrubber_current_page(current_1based as i32);
    // `preview_is_double` resolves the trailing page using the SAME layout as the
    // body (and is decode-free), so it is the exact "current spread has a trailing
    // page" predicate without re-running `current_spread`'s decode.
    let is_double = state.preview_is_double(state.index());
    ui.set_scrubber_double(is_double);
    ui.set_page_jump_text(format!("{}", current_1based).into());
}

/// Returns the current 1-based page number (0 when no pages loaded).
fn current_page_1based(state: &ViewerState) -> usize {
    if state.page_count() == 0 {
        0
    } else {
        state.index() + 1
    }
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

/// Mirror the GLOBAL `Settings` view modes into the runtime (`ViewerState` for
/// direction/spread/cover, `ViewportState` for fit) — the inverse of
/// `reconcile_settings`. Used when the dialog edits the global defaults
/// (opening Library settings) and when resetting an open book to global.
///
/// Borrow discipline: the shared `settings.borrow()` (`s`) is held while each
/// `borrow_mut()` runs, which is safe because `settings`, `state`, and
/// `viewport` are distinct `RefCell`s; one `borrow_mut()` per statement so no
/// two mutable borrows of the same cell overlap.
pub(crate) fn apply_global_view_to_runtime(
    settings: &Rc<RefCell<Settings>>,
    state: &Rc<RefCell<ViewerState>>,
    viewport: &Rc<RefCell<ViewportState>>,
) {
    let s = settings.borrow();
    state
        .borrow_mut()
        .set_reading_direction(s.reading_direction);
    state.borrow_mut().set_spread_mode(s.spread_mode);
    state.borrow_mut().set_cover_mode(s.cover_mode);
    viewport.borrow_mut().set_fit(s.fit_mode);
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

/// Pure helper: decide what view override to write back for the open book.
///
/// Returns `Some((canonical_path, override))` when a book is open (so the
/// caller persists it), `None` otherwise. The override carries all four current
/// runtime modes as `Some(_)`: while a book is open, ITS modes are authoritative,
/// so a write-back fully pins them (a later "reset to global" clears them again).
/// Extracted (mirrors `position_to_write_back`) so the predicate is unit-tested
/// without the effectful `set_overrides` + `save`.
fn view_override_to_write_back(
    open_file: Option<&std::path::Path>,
    reading_direction: ReadingDirection,
    spread_mode: gashuu_core::SpreadMode,
    cover_mode: gashuu_core::CoverMode,
    fit_mode: FitMode,
) -> Option<(std::path::PathBuf, ViewOverride)> {
    open_file.map(|p| {
        (
            p.to_path_buf(),
            ViewOverride {
                reading_direction: Some(reading_direction),
                spread_mode: Some(spread_mode),
                cover_mode: Some(cover_mode),
                fit_mode: Some(fit_mode),
            },
        )
    })
}

/// Write the current runtime view modes back to the OPEN book's override and
/// persist. Called at every viewer leave point (↑ to Library, opening a
/// different book, app exit) and on viewer-context settings-dialog close, so a
/// bare keyboard toggle (D/R/C/fit) persists per-book without opening the dialog.
/// No-op when no book is open.
///
/// Borrow discipline (mirrors `write_back_position`): the `state`/`viewport`
/// shared borrows are confined to the leading block expression and drop before
/// `library.borrow_mut()`. `state` and `viewport` are distinct `RefCell`s, so
/// holding shared borrows of both at once is fine.
pub(crate) fn write_back_view_override(
    state: &Rc<RefCell<ViewerState>>,
    viewport: &Rc<RefCell<ViewportState>>,
    library: &Rc<RefCell<Library>>,
) {
    let Some((path, overrides)) = ({
        let s = state.borrow();
        view_override_to_write_back(
            s.open_file(),
            s.reading_direction(),
            s.spread_mode(),
            s.cover_mode(),
            viewport.borrow().fit_mode(),
        )
    }) else {
        return; // no book open — nothing to write back
    };
    library.borrow_mut().set_overrides(&path, overrides);
    if let Err(e) = library.borrow().save() {
        tracing::error!(error = %e, "failed to save library on view-override write-back");
    }
}

/// Add every path in `paths` to `lib`. Each path is added via `Library::add`,
/// which canonicalizes, dedups, and re-sorts the library on insert.
/// Returns the canonical paths of the books actually inserted (new books only);
/// duplicates within the batch and paths already present are skipped, since
/// `Library::add` returns `None` in those cases.
fn add_paths(lib: &mut Library, paths: Vec<std::path::PathBuf>) -> Vec<std::path::PathBuf> {
    paths
        .into_iter()
        .filter_map(|path| lib.add(path).map(std::path::Path::to_path_buf))
        .collect()
}

/// Return the 0-based VISIBLE (filtered) carousel row index of `path`, or `None`
/// if `path` is not among the currently visible rows. `visible_indices` is the
/// search state's projection (library rows in natural order that pass the filter
/// or are forced visible); the returned position is the index INTO that slice,
/// i.e. the carousel row to focus. `path` must be a canonical path as returned by
/// `add_paths`.
fn visible_focus_index_for_path(
    lib: &Library,
    visible_indices: &[usize],
    path: &std::path::Path,
) -> Option<usize> {
    visible_indices.iter().position(|&library_index| {
        lib.books()
            .get(library_index)
            .is_some_and(|book| book.path() == path)
    })
}

/// Project the CURRENT (already-recomputed) search state into the carousel:
/// rebuild + bind the filtered carousel model, optionally reset carousel focus
/// to row 0, and (re)start cover loading for the visible rows. The SINGLE place
/// the carousel + cover stream are refreshed from the shared search state, shared
/// by the initial boot build, the debounced query callback, and the add path.
///
/// This function only READS `visible_indices()`; it does NOT recompute. Every
/// caller mutates the search state through a recomputing entry point first —
/// `set_query` (startup seed + search-changed) or `force_visible` (add) — so the
/// visible set is already consistent here, avoiding a redundant double-recompute.
///
/// Borrow discipline: all reads share ONE `library.borrow()` scope that drops
/// before the UI bind and `covers.start` (which takes a `borrow_mut` to persist
/// any prefetched page counts) — never hold a `borrow()` across `start`.
fn refresh_library_carousel(
    ui: &ViewerWindow,
    library: &Rc<RefCell<Library>>,
    covers: &cover_loader::CoverController,
    search: &Rc<RefCell<LibrarySearchState>>,
    reset_focus: bool,
) {
    // Read everything the refresh needs under a single borrow, then drop it
    // before the UI mutations and `covers.start`.
    let (book_count, model, cover_reqs) = {
        let lib = library.borrow();
        let indices = search.borrow().visible_indices().to_vec();
        (
            lib.books().len() as i32,
            build_carousel_model(&lib, &indices),
            cover_requests(&lib, &indices),
        )
    };

    ui.set_library_book_count(book_count);
    bind_carousel_model(ui, model);
    if reset_focus {
        ui.set_carousel_focused_index(0);
    }
    covers.start(ui.as_weak(), library, cover_reqs);
}

/// Add `paths` to the library, persist, rebuild the filtered carousel, and
/// surface the outcome on the status line, restoring carousel focus in every
/// case.
///
/// Shared by the Add Files and Add Folder handlers; `op` distinguishes the two
/// only in the save-failure trace message. When nothing new is added there is
/// nothing to persist or rebuild, so it short-circuits after the status update.
///
/// Newly added books are FORCED visible under the active filter (so an add never
/// silently hides the new book behind a non-matching query); the filter text
/// stays in place, and the forced override is cleared on the next user query
/// change (see `LibrarySearchState::set_query`).
fn add_books_and_refresh(
    ui: &ViewerWindow,
    library: &Rc<RefCell<Library>>,
    covers: &cover_loader::CoverController,
    search: &Rc<RefCell<LibrarySearchState>>,
    paths: Vec<std::path::PathBuf>,
    op: &'static str,
) {
    let added_paths = add_paths(&mut library.borrow_mut(), paths);
    if added_paths.is_empty() {
        // Everything picked was already in the library: nothing to persist or rebuild.
        ui.set_status_text("Already in library \u{2014} no new books added.".into());
        ui.invoke_focus_carousel();
        return;
    }
    // Rebuild from the in-memory state even if the save fails, so the newly added
    // books are visible; the save error is then surfaced (not just traced). Keep
    // the just-added paths visible under the active filter, then refresh through
    // the shared chokepoint (which recomputes the filter, rebuilds + binds the
    // model, and restarts the cover stream). Focus is set explicitly below to the
    // new book's visible row, so do NOT reset focus to 0 here.
    let save_result = library.borrow().save();
    // `search` and `library` are distinct RefCells, so the mut borrow of one and
    // the shared borrow of the other cannot conflict; the `library.borrow()`
    // drops at the `;` before refresh. `force_visible` recomputes internally, so
    // the visible set is consistent before `refresh_library_carousel` reads it.
    search
        .borrow_mut()
        .force_visible(added_paths.clone(), &library.borrow());
    refresh_library_carousel(ui, library, covers, search, false);
    match save_result {
        Err(e) => {
            tracing::error!(error = %e, "failed to save library after {op}");
            ui.set_status_text(
                format!(
                    "Added {} book(s), but could not save library: {e}",
                    added_paths.len()
                )
                .into(),
            );
        }
        Ok(()) => {
            ui.set_status_text(format!("Added {} book(s)", added_paths.len()).into());
        }
    }
    // Focus the first newly added book by its VISIBLE row (the carousel renders
    // the filtered slice), not its full-library index.
    if let Some(first_path) = added_paths.first() {
        let index = {
            let lib = library.borrow();
            let search = search.borrow();
            visible_focus_index_for_path(&lib, search.visible_indices(), first_path)
        };
        if let Some(index) = index {
            ui.set_carousel_focused_index(index as i32);
        } else {
            // force_visible(added_paths) + recompute guarantees the just-added book is
            // a visible row, so this is unreachable in practice. Fail loudly in dev/test;
            // in release, log and fall through (focus stays on the carousel via the
            // unconditional invoke_focus_carousel below).
            debug_assert!(
                false,
                "add: forced-visible book {} not found in visible rows",
                first_path.display()
            );
            tracing::warn!(
                path = %first_path.display(),
                "add: forced-visible book not found in visible rows; focus not restored"
            );
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

    // ---- view_override_to_write_back (per-book overrides) ------------------

    #[test]
    fn view_override_to_write_back_none_when_no_open_file() {
        assert!(
            view_override_to_write_back(
                None,
                ReadingDirection::Ltr,
                gashuu_core::SpreadMode::Single,
                gashuu_core::CoverMode::Standalone,
                FitMode::Whole,
            )
            .is_none(),
            "no open file => no write-back"
        );
    }

    #[test]
    fn view_override_to_write_back_some_carries_all_four_modes() {
        use std::path::PathBuf;
        let path = PathBuf::from("/manga/book.cbz");
        let result = view_override_to_write_back(
            Some(path.as_path()),
            ReadingDirection::Rtl,
            gashuu_core::SpreadMode::Double,
            gashuu_core::CoverMode::Paired,
            FitMode::Actual,
        );
        let (p, ov) = result.expect("open file => write-back tuple");
        assert_eq!(p, path);
        assert_eq!(ov.reading_direction, Some(ReadingDirection::Rtl));
        assert_eq!(ov.spread_mode, Some(gashuu_core::SpreadMode::Double));
        assert_eq!(ov.cover_mode, Some(gashuu_core::CoverMode::Paired));
        assert_eq!(ov.fit_mode, Some(FitMode::Actual));
    }

    #[test]
    fn add_paths_empty_vec_returns_zero() {
        let mut lib = gashuu_core::Library::new();
        let added = add_paths(&mut lib, vec![]);
        assert!(added.is_empty());
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
        assert_eq!(added.len(), 2);
        assert_eq!(lib.books().len(), 2);
        // Nonexistent paths cannot be canonicalized, so `Library::add` stores the
        // verbatim path; the returned vec therefore equals the input, in input order.
        assert_eq!(
            added,
            vec![
                std::path::PathBuf::from("nonexistent/vol1.cbz"),
                std::path::PathBuf::from("nonexistent/vol2.cbz"),
            ]
        );
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
            added.len(),
            1,
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
            added.len(),
            1,
            "a path already in the library must not be counted"
        );
        assert_eq!(lib.books().len(), 2);
    }

    #[test]
    fn add_paths_returns_canonical_paths_and_skips_duplicates() {
        let mut lib = gashuu_core::Library::new();
        let root = tempfile::tempdir().expect("tempdir");
        let first = root.path().join(".");
        let expected = root.path().canonicalize().expect("canonicalize tempdir");
        let added = add_paths(&mut lib, vec![first.clone(), first.clone()]);
        assert_eq!(added, vec![expected.clone()]);
        assert_eq!(lib.books().len(), 1);
        assert_eq!(lib.books()[0].path(), expected.as_path());
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
        assert!(added.is_empty(), "all-duplicate batch must return 0");
        assert_eq!(lib.books().len(), before, "books count must not change");
    }

    #[test]
    fn visible_focus_index_for_path_uses_filtered_rows() {
        let mut lib = Library::new();
        assert!(lib
            .add(std::path::PathBuf::from("/manga/alpha.cbz"))
            .is_some());
        assert!(lib
            .add(std::path::PathBuf::from("/manga/beta.cbz"))
            .is_some());
        assert!(lib
            .add(std::path::PathBuf::from("/manga/gamma.cbz"))
            .is_some());

        let path = lib.books()[2].path().to_path_buf();
        // gamma is the only visible row, so its visible position is 0.
        assert_eq!(visible_focus_index_for_path(&lib, &[2], &path), Some(0));
        // gamma is not among the visible rows, so it has no visible position.
        assert_eq!(visible_focus_index_for_path(&lib, &[0, 1], &path), None);
    }

    #[test]
    fn add_paths_returns_input_order_while_books_are_natural_order() {
        // Focus follows the FIRST input path, not natural order: `add_paths`
        // returns the inserted paths in INPUT order, whereas `lib.books()` keeps
        // them in NATURAL (sorted) order. Nonexistent paths cannot be
        // canonicalized, so `Library::add` falls back to the verbatim path and the
        // returned paths equal the verbatim input paths.
        let mut lib = gashuu_core::Library::new();
        let vol10 = std::path::PathBuf::from("nonexistent/vol10.cbz");
        let vol1 = std::path::PathBuf::from("nonexistent/vol1.cbz");
        let added = add_paths(&mut lib, vec![vol10.clone(), vol1.clone()]);

        // Returned vec is in INPUT order (vol10 first, vol1 second).
        assert_eq!(added[0], vol10);
        assert_eq!(added[1], vol1);

        // The library itself is in NATURAL order (vol1 before vol10).
        let books: Vec<_> = lib
            .books()
            .iter()
            .map(|book| book.path().to_path_buf())
            .collect();
        assert_eq!(books, vec![vol1, vol10]);
    }

    // Note: `build_carousel_model` is now headless (it builds the model from
    // visible indices; `bind_carousel_model` does the UI bind), and is unit-tested
    // directly in `carousel::tests`. The Library -> carousel row mapping invariants
    // (length, 1-based `current`, availability, natural `Library::books()` order)
    // are covered by `library_model::tests` against the pure `carousel_data` /
    // `carousel_data_for_indices` helpers that the builder delegates to.
}
