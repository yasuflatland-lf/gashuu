use crate::enum_adapters::{
    cover_mode_to_index, fit_mode_to_index, index_to_cover_mode, index_to_fit_mode,
    index_to_language, index_to_reading_direction, index_to_spread_mode, language_to_index,
    reading_direction_to_index, spread_mode_to_index,
};
use crate::library_model::{LibrarySearchState, LibrarySelectionState};
use crate::page_loader::PageController;
use crate::viewer_state::ViewerState;
use crate::viewport::ViewportState;
use crate::{
    apply_global_view_to_runtime, cover_loader, i18n, persist_view_modes, push_selection_strings,
    refresh, refresh_library_carousel, report_save_error, with_ui, CarouselRefresh, ViewModeRoute,
    ViewerWindow,
};
use gashuu_core::{CacheConfig, Library, Settings, ThumbnailCache, ViewOverride};
use slint::ComponentHandle;
use std::cell::RefCell;
use std::rc::Rc;

/// Registers the settings/shortcuts dialog lifecycle callbacks (open, close,
/// shortcuts overlay open/close, reset overrides, first-run guide dismissal,
/// and the immediate data-clearing actions) onto `ui`.
/// Panel constraint (#151): explicit handle list IS the dependency list — no AppState bundle.
/// `covers`/`search`/`selection` are threaded in for the clear-reading-history
/// refresh, which rebuilds the library carousel after emptying the library.
#[allow(clippy::too_many_arguments)]
pub(crate) fn wire_settings_handlers(
    ui: &ViewerWindow,
    state: &Rc<RefCell<ViewerState>>,
    viewport: &Rc<RefCell<ViewportState>>,
    settings: &Rc<RefCell<Settings>>,
    library: &Rc<RefCell<Library>>,
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
    let covers = Rc::clone(covers);
    let pages = Rc::clone(pages);
    let search = Rc::clone(search);
    let selection = Rc::clone(selection);
    let localizer = Rc::clone(localizer);

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
        let localizer = Rc::clone(&localizer);
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
                ui.set_allow_rar_archives(s.allow_rar_archives);
                // Clear any stale data-clearing status from a prior open so the
                // feedback line starts hidden each time the dialog opens.
                ui.set_data_action_status("".into());
                ui.set_language_index(language_to_index(s.language));
                ui.set_key_bindings_text(
                    crate::i18n::dynamic::shortcuts_help(localizer.loader()).into(),
                );
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
    // `persist_view_modes` takes `&Rc` handles and confines every `borrow()` /
    // `borrow_mut()` inside the call, so no borrow outlives it — the following
    // `settings.borrow().save()` always gets a fresh, unconflicted borrow.
    {
        let ui_weak = ui.as_weak();
        let state = Rc::clone(&state);
        let settings = Rc::clone(&settings);
        let viewport = Rc::clone(&viewport);
        let library = Rc::clone(&library);
        let localizer = Rc::clone(&localizer);
        ui.on_close_settings(move || {
            with_ui(&ui_weak, |ui| {
                ui.set_show_settings(false);
                // screen 0 = Library (edits GLOBAL defaults), 1 = Viewer (edits the
                // CURRENT book's per-book override). Routing lives in
                // `persist_view_modes` (ADR-0007 clobber-trap).
                if ui.get_screen() == 0 {
                    persist_view_modes(
                        ViewModeRoute::DialogClosedOnLibrary,
                        &state,
                        &viewport,
                        &settings,
                        &library,
                    );
                    if let Err(e) = settings.borrow().save() {
                        report_save_error(
                            &ui,
                            localizer.loader(),
                            &e,
                            "failed to save settings from dialog",
                        );
                    }
                    ui.invoke_focus_carousel();
                } else {
                    // Persist the four view modes to this book's override. The
                    // cache/preload/track fields are global; save Settings too so a
                    // change to them in the viewer dialog is not lost (the view-mode
                    // fields in Settings are untouched because we did NOT reconcile).
                    persist_view_modes(
                        ViewModeRoute::DialogClosedOnViewer,
                        &state,
                        &viewport,
                        &settings,
                        &library,
                    );
                    if let Err(e) = settings.borrow().save() {
                        report_save_error(
                            &ui,
                            localizer.loader(),
                            &e,
                            "failed to save settings from dialog",
                        );
                    }
                    ui.invoke_focus_pages();
                }
            })
        });
    }

    // Show the shortcuts overlay: the overlay opens on top of the still-open settings
    // dialog. Load-bearing assumption: this handler is only reachable via the settings
    // footer link, so on_open_settings has always run first and populated
    // key_bindings_text. A future entry point that bypasses settings MUST set
    // key_bindings_text itself before opening the overlay.
    // Focus management is intentionally omitted: ShortcutsOverlay's `init` grabs
    // focus itself on appear, so no explicit focus call is needed here.
    {
        let ui_weak = ui.as_weak();
        ui.on_open_shortcuts(move || {
            with_ui(&ui_weak, |ui| {
                ui.set_show_shortcuts(true);
            })
        });
    }

    // Close the shortcuts overlay: hide it and return focus to the still-mounted
    // SettingsDialog.  Focus must not go to the screen behind — the dialog remains
    // open.  invoke_focus_settings drives a focus epoch on the dialog because
    // if-gated child elements cannot be targeted directly.
    // Closing the overlay must NOT close settings: do not touch show_settings and
    // do not run reconcile/save here.
    {
        let ui_weak = ui.as_weak();
        ui.on_close_shortcuts(move || {
            with_ui(&ui_weak, |ui| {
                ui.set_show_shortcuts(false);
                // Today the overlay is only reachable while the settings dialog is
                // mounted, so this branch is always taken.  The guard keeps focus
                // restoration from silently no-oping if a future entry point opens
                // the overlay without settings.
                if ui.get_show_settings() {
                    ui.invoke_focus_settings();
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
        let localizer = Rc::clone(&localizer);
        ui.on_reset_overrides(move || {
            with_ui(&ui_weak, |ui| {
                if let Some(path) = state.borrow().open_file().map(|p| p.to_path_buf()) {
                    let changed = library.borrow_mut().set_overrides(&path, ViewOverride::none());
                    if !changed {
                        tracing::warn!(path = %path.display(), "reset override: open book not found in library");
                    }
                    if let Err(e) = library.borrow().save() {
                        report_save_error(&ui, localizer.loader(), &e, "failed to save library on override reset");
                    }
                }
                // Apply the global defaults to the runtime + view.
                apply_global_view_to_runtime(&settings, &state, &viewport);
                refresh(
                    &ui,
                    &state.borrow(),
                    &viewport,
                    localizer.loader(),
                    &pages,
                    ui.as_weak(),
                );
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

    // Clear reading history (issue #178): immediate, no confirmation. Empties the
    // library (books + last_opened) and the recent-files list, persists both, then
    // rebuilds the library carousel IN PLACE. The viewer / current screen is NOT
    // touched: a book open in this session stays open — only the persisted history
    // and the library carousel are cleared. Localized status feedback is pushed to
    // `data-action-status`.
    {
        let ui_weak = ui.as_weak();
        let settings = Rc::clone(&settings);
        let library = Rc::clone(&library);
        let covers = Rc::clone(&covers);
        let search = Rc::clone(&search);
        let selection = Rc::clone(&selection);
        let localizer = Rc::clone(&localizer);
        ui.on_clear_reading_history(move || {
            with_ui(&ui_weak, |ui| {
                // Mutate then save under tight borrow scopes that drop before the
                // refresh below (which re-borrows library/search/selection/settings).
                library.borrow_mut().clear();
                settings.borrow_mut().recent_files.clear();
                // Save library and settings INDEPENDENTLY so each failure is
                // reported distinctly and a partial success is correctly diagnosed
                // (library cleared and persisted even if settings save fails, or
                // vice-versa). Both borrows drop before the carousel rebuild below.
                let lib_err = library.borrow().save().err();
                let set_err = settings.borrow().save().err();
                if let Some(ref e) = lib_err {
                    tracing::error!(error = %e, "failed to persist library while clearing reading history");
                }
                if let Some(ref e) = set_err {
                    tracing::error!(error = %e, "failed to persist settings while clearing reading history");
                }
                // Recompute the (now empty) search projection and drop the bulk
                // selection, then project into the carousel. In-memory state is
                // already cleared regardless of save outcome, so the UI MUST
                // reflect it — no early-return here. `set_query` recomputes
                // internally; its temporary `library.borrow()` drops at the `;`.
                search
                    .borrow_mut()
                    .set_query(String::new(), &library.borrow());
                selection.borrow_mut().clear();
                refresh_library_carousel(
                    &ui,
                    &CarouselRefresh {
                        library: &library,
                        covers: &covers,
                        search: &search,
                        selection: &selection,
                        localizer: &localizer,
                    },
                    true,
                );
                // Show failure status if either save failed; success only when both
                // persisted cleanly.
                let status = if lib_err.is_some() || set_err.is_some() {
                    i18n::dynamic::reading_history_clear_failed(localizer.loader())
                } else {
                    i18n::dynamic::reading_history_cleared(localizer.loader())
                };
                ui.set_data_action_status(status.into());
            })
        });
    }

    // Clear cover cache (issue #178): immediate, no confirmation. Deletes the
    // on-disk thumbnail cache files; in-session covers already rendered are left
    // alone (no carousel rebuild). A best-effort report drives the localized
    // status; a failure to open the cache directory surfaces a failure status.
    {
        let ui_weak = ui.as_weak();
        let localizer = Rc::clone(&localizer);
        ui.on_clear_cover_cache(move || {
            with_ui(&ui_weak, |ui| match ThumbnailCache::new() {
                Ok(cache) => {
                    let report = cache.clear();
                    ui.set_data_action_status(
                        i18n::dynamic::cover_cache_cleared(
                            localizer.loader(),
                            report.removed_files,
                            report.removed_bytes,
                        )
                        .into(),
                    );
                }
                Err(e) => {
                    tracing::error!(error = %e, "failed to open thumbnail cache for clearing");
                    ui.set_data_action_status(
                        i18n::dynamic::cover_cache_clear_failed(localizer.loader()).into(),
                    );
                }
            })
        });
    }
}

/// Registers the view-mode and preference setter callbacks (reading direction,
/// spread, cover, fit, cache size, preload, recents tracking, language) onto `ui`.
/// Panel constraint (#151): explicit handle list IS the dependency list — no AppState bundle.
#[allow(clippy::too_many_arguments)]
pub(crate) fn wire_view_mode_handlers(
    ui: &ViewerWindow,
    state: &Rc<RefCell<ViewerState>>,
    viewport: &Rc<RefCell<ViewportState>>,
    settings: &Rc<RefCell<Settings>>,
    library: &Rc<RefCell<Library>>,
    pages: &Rc<PageController>,
    search: &Rc<RefCell<LibrarySearchState>>,
    selection: &Rc<RefCell<LibrarySelectionState>>,
    localizer: &Rc<i18n::Localizer>,
) {
    let state = Rc::clone(state);
    let viewport = Rc::clone(viewport);
    let settings = Rc::clone(settings);
    let library = Rc::clone(library);
    let pages = Rc::clone(pages);
    let search = Rc::clone(search);
    let selection = Rc::clone(selection);
    let localizer = Rc::clone(localizer);

    // Settings setters (one per dialog control). Mode/cover/direction are made
    // idempotent by their `ViewerState` value setters returning a "changed" bool;
    // fit uses an explicit equality guard. The borrow discipline mirrors the
    // `ToggleSpread` handler: the temporary `borrow_mut()` in the `if` condition
    // drops before the block runs, so `refresh(&ui, &state.borrow(), ..)` is safe.
    {
        let ui_weak = ui.as_weak();
        let state = Rc::clone(&state);
        let viewport = Rc::clone(&viewport);
        let pages = Rc::clone(&pages);
        let localizer = Rc::clone(&localizer);
        ui.on_set_reading_direction(move |i| {
            with_ui(&ui_weak, |ui| {
                let dir = index_to_reading_direction(i);
                // Mutates the runtime view mode only; while a book is open this change
                // is persisted to the current book's per-book override via
                // `write_back_view_override` at the next viewer leave point, not into
                // the global `Settings`.
                if state.borrow_mut().set_reading_direction(dir) {
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
    {
        let ui_weak = ui.as_weak();
        let state = Rc::clone(&state);
        let viewport = Rc::clone(&viewport);
        let pages = Rc::clone(&pages);
        let localizer = Rc::clone(&localizer);
        ui.on_set_spread_mode(move |i| {
            with_ui(&ui_weak, |ui| {
                let mode = index_to_spread_mode(i);
                // Mutates the runtime view mode only; while a book is open this change
                // is persisted to the current book's per-book override via
                // `write_back_view_override` at the next viewer leave point, not into
                // the global `Settings`.
                if state.borrow_mut().set_spread_mode(mode) {
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
    {
        let ui_weak = ui.as_weak();
        let state = Rc::clone(&state);
        let viewport = Rc::clone(&viewport);
        let pages = Rc::clone(&pages);
        let localizer = Rc::clone(&localizer);
        ui.on_set_cover_mode(move |i| {
            with_ui(&ui_weak, |ui| {
                let mode = index_to_cover_mode(i);
                // Mutates the runtime view mode only; while a book is open this change
                // is persisted to the current book's per-book override via
                // `write_back_view_override` at the next viewer leave point, not into
                // the global `Settings`.
                if state.borrow_mut().set_cover_mode(mode) {
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
    {
        let ui_weak = ui.as_weak();
        let state = Rc::clone(&state);
        let viewport = Rc::clone(&viewport);
        let pages = Rc::clone(&pages);
        let localizer = Rc::clone(&localizer);
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
    {
        let state = Rc::clone(&state);
        let settings = Rc::clone(&settings);
        // Cache size applies to newly opened books; no refresh of the current view.
        ui.on_set_cache_size(move |v| {
            // Read the current preload while writing cache_size, then mirror both
            // into ViewerState so the next opened book picks up the change this
            // session. `max(1)` guards the i32->usize cast against a negative
            // stepper value; `CacheConfig::new` owns the upper clamp, and reading
            // `capacity()` back keeps the persisted field equal to the value used.
            let preload = settings.borrow().preload_pages;
            let cfg = CacheConfig::new(v.max(1) as usize, preload);
            settings.borrow_mut().cache_size = cfg.capacity();
            state.borrow_mut().set_cache_config(cfg);
        });
    }
    {
        let state = Rc::clone(&state);
        let settings = Rc::clone(&settings);
        // Preload radius applies to newly opened books; no refresh. 0 is a valid
        // "prefetch disabled" radius. `max(0)` guards the i32->usize cast; the
        // upper clamp and the floor live in `CacheConfig::new`, and reading
        // `radius()` back keeps the persisted field equal to the value used.
        ui.on_set_preload_pages(move |v| {
            let cache_size = settings.borrow().cache_size;
            let cfg = CacheConfig::new(cache_size, v.max(0) as usize);
            settings.borrow_mut().preload_pages = cfg.radius();
            state.borrow_mut().set_cache_config(cfg);
        });
    }
    {
        let settings = Rc::clone(&settings);
        ui.on_set_track_recent(move |b| {
            settings.borrow_mut().track_recent_files = b;
        });
    }
    {
        let settings = Rc::clone(&settings);
        ui.on_set_allow_rar_archives(move |b| {
            settings.borrow_mut().allow_rar_archives = b;
        });
    }
    {
        let ui_weak = ui.as_weak();
        let state = Rc::clone(&state);
        let settings = Rc::clone(&settings);
        let viewport = Rc::clone(&viewport);
        let pages = Rc::clone(&pages);
        let localizer = Rc::clone(&localizer);
        let library = Rc::clone(&library);
        let search = Rc::clone(&search);
        let selection = Rc::clone(&selection);
        ui.on_set_language(move |i| {
            with_ui(&ui_weak, |ui| {
                let lang = index_to_language(i);
                // Mirror into the runtime state (the same dual-write the
                // cache-size handler does); the idempotent setter absorbs the
                // dropdown's selection self-fire. Persisting happens at the
                // dialog's close path, like every other global field.
                if !state.borrow_mut().set_language(lang) {
                    return;
                }
                settings.borrow_mut().language = lang;
                // Reload the Fluent catalog for the new language.
                // Deliberate loud-panic policy: compile-time-embedded catalogs
                // and exhaustive langid_for make a load failure theoretically
                // unreachable; a panic surfaces programmer error immediately.
                localizer.switch(lang);
                // Push the newly loaded catalog into the Strings global so
                // every Fluent-sourced label flips to the new language atomically
                // before the next paint.
                localizer.apply(&ui);
                ui.set_key_bindings_text(
                    crate::i18n::dynamic::shortcuts_help(localizer.loader()).into(),
                );
                refresh(
                    &ui,
                    &state.borrow(),
                    &viewport,
                    localizer.loader(),
                    &pages,
                    ui.as_weak(),
                );
                // Recompose the selection-toolbar strings in the new language.
                push_selection_strings(&ui, &localizer, &selection, &search, &library);
                // Recompose the library-count idle strip label too: the language
                // switch does not run `refresh_library_carousel`, so the count
                // string must be re-pushed here from the current book count.
                ui.set_library_count_text(
                    crate::i18n::dynamic::library_count_text(
                        localizer.loader(),
                        library.borrow().books().len(),
                    )
                    .into(),
                );
            })
        });
    }
}
