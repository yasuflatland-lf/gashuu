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
    apply_global_view_to_runtime, cover_loader, current_runtime_view, i18n,
    push_selection_toolbar_state, refresh, refresh_library_carousel, report_save_error,
    route_view_modes_to_sink, with_ui, CarouselRefresh, ViewModeRoute, ViewerWindow,
};
use gashuu_core::{CacheConfig, Library, Settings, ThumbnailCache, ViewOverride};
use slint::ComponentHandle;
use std::cell::RefCell;
use std::rc::Rc;

/// Registers the settings/shortcuts dialog lifecycle callbacks (open, close,
/// shortcuts overlay open/close, reset overrides,
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

    // Snapshot of the open book's runtime view, taken by `on_open_settings` just
    // before the Library-screen dialog seeds the SHARED runtime with global
    // defaults, and consumed by `on_close_settings` to restore it (issue #414).
    // Some only while a book was open at Library-dialog-open time; None otherwise,
    // so a pure-Library dialog (no book open) and the Viewer branch both no-op.
    let pre_dialog_view: Rc<RefCell<Option<gashuu_core::ResolvedView>>> =
        Rc::new(RefCell::new(None));

    // Open the settings dialog. Display modes are read from the RUNTIME source of truth
    // (state/viewport) so it never shows a stale value; cache/preload/track from Settings.
    {
        let ui_weak = ui.as_weak();
        let state = Rc::clone(&state);
        let settings = Rc::clone(&settings);
        let viewport = Rc::clone(&viewport);
        let localizer = Rc::clone(&localizer);
        let pre_dialog_view = Rc::clone(&pre_dialog_view);
        ui.on_open_settings(move || {
            with_ui(&ui_weak, |ui| {
                // screen 1 = Viewer (per-book), screen 0 = Library (global defaults).
                let per_book = ui.get_screen() == 1;
                // On the Library screen the dialog edits GLOBAL defaults, so mirror them
                // into the runtime first (it seeds from there); a book re-applies its override.
                if !per_book {
                    // Before seeding the SHARED runtime with global defaults, snapshot the
                    // open book's runtime so `on_close_settings` can restore it (issue #414);
                    // otherwise this seed clobbers the book's runtime and the later leave/exit
                    // write-back would persist the global values as the book's override.
                    // None when no book is open, so a pure-Library dialog has nothing to restore.
                    let has_book = state.borrow().open_file().is_some();
                    *pre_dialog_view.borrow_mut() =
                        has_book.then(|| current_runtime_view(&state, &viewport));
                    apply_global_view_to_runtime(&settings, &state, &viewport);
                }
                let s = settings.borrow();
                let st = state.borrow();
                ui.set_reading_direction_index(reading_direction_to_index(st.reading_direction()));
                ui.set_spread_mode_index(spread_mode_to_index(st.spread_mode()));
                ui.set_cover_mode_index(cover_mode_to_index(st.cover_mode()));
                // Fit mode is owned by the viewport at runtime.
                ui.set_fit_mode_index(fit_mode_to_index(viewport.borrow().fit_mode()));
                ui.set_cache_size(s.cache_capacity as i32);
                ui.set_preload_pages(s.prefetch_radius as i32);
                ui.set_track_recent(s.track_recent_sources);
                ui.set_allow_rar_archives(s.allow_rar_archives);
                // Clear any stale data-clearing status from a prior open so the
                // feedback line starts hidden each time the dialog opens.
                ui.set_data_action_status("".into());
                // Clear the manual "Check for updates now" feedback line too, else a stale
                // "You're on the latest version." would persist across close/reopen.
                ui.set_settings_update_status(Default::default());
                ui.set_language_index(language_to_index(s.language));
                ui.set_key_bindings_text(
                    crate::i18n::dynamic::shortcuts_help(localizer.loader()).into(),
                );
                ui.set_settings_per_book(per_book);
                ui.set_show_settings(true);
            })
        });
    }

    // Close the settings dialog: hide, reconcile modes into Settings, persist, then
    // restore focus to the matching FocusScope (screen 0 Library, 1 Viewer) or keys die.
    {
        let ui_weak = ui.as_weak();
        let state = Rc::clone(&state);
        let settings = Rc::clone(&settings);
        let viewport = Rc::clone(&viewport);
        let library = Rc::clone(&library);
        let localizer = Rc::clone(&localizer);
        let pre_dialog_view = Rc::clone(&pre_dialog_view);
        ui.on_close_settings(move || {
            with_ui(&ui_weak, |ui| {
                ui.set_show_settings(false);
                // screen 0 = Library (GLOBAL defaults), 1 = Viewer (per-book override).
                // Routing lives in `route_view_modes_to_sink` (ADR-0007 clobber-trap).
                if ui.get_screen() == 0 {
                    route_view_modes_to_sink(
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
                    // Restore the open book's runtime that the dialog overwrote with global
                    // defaults on open (issue #414). The global reconcile+save above already
                    // captured any Library-screen edits, so putting the book's own runtime
                    // back makes the later leave/exit write-back pin the BOOK's value instead
                    // of the transiently-global one. No-op when no book was open at open time.
                    if let Some(v) = pre_dialog_view.borrow_mut().take() {
                        state.borrow_mut().apply_resolved_view(v);
                        viewport.borrow_mut().set_fit(v.fit_mode);
                    }
                    ui.invoke_focus_carousel();
                } else {
                    // Persist the four view modes to this book's override. cache/preload/track
                    // are global, so save Settings too (its view-mode fields stay untouched).
                    route_view_modes_to_sink(
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

    // Load-bearing assumption: reachable only via the settings footer link, so
    // on_open_settings already populated key_bindings_text. A bypassing entry point MUST set it.
    {
        let ui_weak = ui.as_weak();
        ui.on_open_shortcuts(move || {
            with_ui(&ui_weak, |ui| {
                ui.set_show_shortcuts(true);
            })
        });
    }

    // Close the shortcuts overlay: return focus to the still-mounted SettingsDialog via a
    // focus epoch (if-gated elements can't be targeted directly). Must NOT close settings.
    {
        let ui_weak = ui.as_weak();
        ui.on_close_shortcuts(move || {
            with_ui(&ui_weak, |ui| {
                ui.set_show_shortcuts(false);
                // The overlay is only reachable while settings is mounted (branch always
                // taken); the guard keeps focus restore from no-oping if that changes.
                if ui.get_show_settings() {
                    ui.invoke_focus_settings();
                }
            })
        });
    }

    // Reset-to-global (viewer only): clear THIS book's override to inherit global
    // defaults, apply to the live view, and re-seed the dialog's combos.
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
                // Guard the on-close write-back (#415): keep this book's override
                // EMPTY (inherit) until the user changes a view mode again. Without
                // this, `write_back_view_override` would re-pin the four runtime
                // fields on dialog close and instantly undo the reset. Marked AFTER
                // `apply_global_view_to_runtime`, whose setters would otherwise clear it.
                state.borrow_mut().mark_inherit_pending();
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

    // Clear reading history (issue #178): immediate, no confirmation. Empties library +
    // recent-files, persists, rebuilds the carousel IN PLACE; the open viewer is untouched.
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
                settings.borrow_mut().recent_sources.clear();
                // Save library and settings INDEPENDENTLY so each failure is diagnosed
                // distinctly (a partial success is correctly reported).
                let lib_err = library.borrow().save().err();
                let set_err = settings.borrow().save().err();
                if let Some(ref e) = lib_err {
                    tracing::error!(error = %e, "failed to persist library while clearing reading history");
                }
                if let Some(ref e) = set_err {
                    tracing::error!(error = %e, "failed to persist settings while clearing reading history");
                }
                // Recompute the (now empty) search projection, drop the selection, and
                // project into the carousel. In-memory state is cleared regardless of save.
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

    // Clear cover cache (issue #178): immediate. Deletes on-disk thumbnail files;
    // in-session covers are left alone (no rebuild). Best-effort, with status feedback.
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

    // Settings setters (one per control). Mode/cover/direction are idempotent via their
    // "changed" bool; fit uses an equality guard. borrow_mut in the `if` drops before refresh.
    {
        let ui_weak = ui.as_weak();
        let state = Rc::clone(&state);
        let viewport = Rc::clone(&viewport);
        let pages = Rc::clone(&pages);
        let localizer = Rc::clone(&localizer);
        ui.on_set_reading_direction(move |i| {
            with_ui(&ui_weak, |ui| {
                let dir = index_to_reading_direction(i);
                // Mutates the runtime view mode only; while a book is open it persists to
                // the book's per-book override via `write_back_view_override`, not Settings.
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
                // Mutates the runtime view mode only; while a book is open it persists to
                // the book's per-book override via `write_back_view_override`, not Settings.
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
                // Mutates the runtime view mode only; while a book is open it persists to
                // the book's per-book override via `write_back_view_override`, not Settings.
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
                // Equality guard (viewport setter isn't idempotent-by-return); the viewport
                // owns fit_mode, persisted to the book's per-book override, not Settings.
                if viewport.borrow().fit_mode() != mode {
                    viewport.borrow_mut().set_fit(mode);
                    // Fit lives on ViewportState, so its change can't clear the
                    // inherit-pending guard the way the ViewerState setters do (#415);
                    // clear it here so a fit change after a reset re-enables pinning.
                    state.borrow_mut().clear_inherit_pending();
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
            // Mirror cache_capacity + prefetch radius into ViewerState for newly opened books.
            // `max(1)` guards the cast; reading `capacity()` back keeps the persisted field exact.
            let radius = settings.borrow().prefetch_radius;
            let cfg = CacheConfig::new(v.max(1) as usize, radius);
            settings.borrow_mut().cache_capacity = cfg.capacity();
            state.borrow_mut().set_cache_config(cfg);
        });
    }
    {
        let state = Rc::clone(&state);
        let settings = Rc::clone(&settings);
        // Prefetch radius for newly opened books; 0 = prefetch disabled. `max(0)` guards
        // the cast; reading `radius()` back keeps the persisted field exact.
        ui.on_set_preload_pages(move |v| {
            let cache_capacity = settings.borrow().cache_capacity;
            let cfg = CacheConfig::new(cache_capacity, v.max(0) as usize);
            settings.borrow_mut().prefetch_radius = cfg.radius();
            state.borrow_mut().set_cache_config(cfg);
        });
    }
    {
        let settings = Rc::clone(&settings);
        ui.on_set_track_recent(move |b| {
            settings.borrow_mut().track_recent_sources = b;
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
                // Mirror into the runtime state; the idempotent setter absorbs the dropdown's
                // self-fire. Persisting happens at the dialog's close path, like other globals.
                if !state.borrow_mut().set_language(lang) {
                    return;
                }
                settings.borrow_mut().language = lang;
                // Reload the Fluent catalog. Loud-panic policy: embedded catalogs +
                // exhaustive langid_for make load failure unreachable; a panic surfaces bugs.
                localizer.switch(lang);
                // Push the loaded catalog into the Strings global so every Fluent label
                // flips to the new language before the next paint.
                localizer.push_strings_to_ui(&ui);
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
                push_selection_toolbar_state(&ui, &localizer, &selection, &search, &library);
                // Recompose the library-count label too: the language switch skips
                // `refresh_library_carousel`, so re-push it from the current book count.
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
