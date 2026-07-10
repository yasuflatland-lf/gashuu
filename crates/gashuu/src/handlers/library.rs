use crate::{
    add_controller,
    carousel::{apply_selection_flags, set_carousel_selected},
    cover_loader, i18n, use_cases,
};
use crate::{
    apply_add_report, apply_outcomes, current_book_name, finalize_empty_book_removed,
    finalize_open, finalize_remove, go_to_viewer, push_selection_toolbar_state,
    refresh_library_carousel, visible_index_to_path, with_ui, CarouselRefresh, ViewerWindow,
};
use crate::{
    library_model::{LibrarySearchState, LibrarySelectionState},
    navigation::NavState,
    page_loader::PageController,
    selection_projection,
};
use crate::{viewer_state::ViewerState, viewport::ViewportState};
use gashuu_core::{Library, Settings};
use slint::ComponentHandle;
use std::cell::RefCell;
use std::rc::Rc;

/// Registers the add-books/add-folder callbacks onto `ui`.
/// Panel constraint (#151): explicit handle list IS the dependency list — no AppState bundle.
#[allow(clippy::too_many_arguments)]
pub(crate) fn wire_open_handlers(
    ui: &ViewerWindow,
    // Retained for call-site stability (`main` wires every handler group with the
    // same collaborator list); the add-only paths below no longer touch these.
    _state: &Rc<RefCell<ViewerState>>,
    _viewport: &Rc<RefCell<ViewportState>>,
    settings: &Rc<RefCell<Settings>>,
    library: &Rc<RefCell<Library>>,
    _open_book: &Rc<use_cases::OpenBookUseCase>,
    covers: &Rc<cover_loader::CoverController>,
    _pages: &Rc<PageController>,
    adder: &Rc<add_controller::AddController>,
    search: &Rc<RefCell<LibrarySearchState>>,
    selection: &Rc<RefCell<LibrarySelectionState>>,
    localizer: &Rc<i18n::Localizer>,
) {
    // Rebind the `&Rc<_>` params to owned `Rc` locals so each closure's `Rc::clone(&handle)`
    // prelude stays byte-identical to its pre-extraction form in `main`.
    let settings = Rc::clone(settings);
    let library = Rc::clone(library);
    let covers = Rc::clone(covers);
    let adder = Rc::clone(adder);
    let search = Rc::clone(search);
    let selection = Rc::clone(selection);
    let localizer = Rc::clone(localizer);

    // Add Books: pick comic sources. macOS picks archives AND folders in one panel
    // (`pick_files_or_folders`); sources are probed off the UI thread (issue 206).
    {
        let ui_weak = ui.as_weak();
        let settings = Rc::clone(&settings);
        let adder = Rc::clone(&adder);
        ui.on_add_books(move || {
            with_ui(&ui_weak, |ui| {
                let dialog = rfd::FileDialog::new()
                    .add_filter("Comic archive", &["cbz", "zip", "cbr", "rar"]);
                #[cfg(target_os = "macos")]
                let picked = dialog.pick_files_or_folders();
                #[cfg(not(target_os = "macos"))]
                let picked = dialog.pick_files();
                let Some(paths) = picked else {
                    return;
                };
                let policy = settings.borrow().archive_policy();
                adder.start(ui.as_weak(), paths, policy, "add-books");
            })
        });
    }

    // Add Folder: pick one folder, added as one book. Wraps it in a `vec![]` so it reuses
    // the same off-thread probe + `add-finalize` apply path as `on_add_books` (issue 206).
    {
        let ui_weak = ui.as_weak();
        let settings = Rc::clone(&settings);
        let adder = Rc::clone(&adder);
        ui.on_add_folder(move || {
            with_ui(&ui_weak, |ui| {
                let Some(folder) = rfd::FileDialog::new().pick_folder() else {
                    return;
                };
                let policy = settings.borrow().archive_policy();
                adder.start(ui.as_weak(), vec![folder], policy, "add-folder");
            })
        });
    }

    // Bulk-add progress tick (issue 206): a probe completed; update the status to
    // "Adding… (done/total)". Epoch-guarded, so superseded ticks are already dropped.
    {
        let ui_weak = ui.as_weak();
        let localizer = Rc::clone(&localizer);
        ui.on_add_progress(move |done, total| {
            with_ui(&ui_weak, |ui| {
                let done = done.max(0) as usize;
                let total = total.max(0) as usize;
                // A new add is underway: clear any lingering error toast from the
                // previous add so it doesn't float over this operation's progress.
                ui.set_add_toast_text("".into());
                ui.set_status_text(
                    crate::i18n::dynamic::adding_progress(localizer.loader(), done, total).into(),
                );
                // Show the bottom progress hairline and advance its determinate
                // fill. Epoch-guarded upstream, so only live-generation ticks reach here.
                ui.set_add_active(true);
                ui.set_add_progress_ratio(crate::add_controller::add_progress_ratio(done, total));
            })
        });
    }

    // Bulk-add finalize (issue 206): drain this generation's outcomes (epoch-guarded;
    // `None` = superseded), mutate the library on the UI thread, then run the add tail.
    {
        let ui_weak = ui.as_weak();
        let adder = Rc::clone(&adder);
        let library = Rc::clone(&library);
        let covers = Rc::clone(&covers);
        let search = Rc::clone(&search);
        let selection = Rc::clone(&selection);
        let localizer = Rc::clone(&localizer);
        ui.on_add_finalize(move |epoch| {
            with_ui(&ui_weak, |ui| {
                let Some((outcomes, op)) = adder.take_outcomes(epoch.max(0) as usize) else {
                    return;
                };
                let report = apply_outcomes(&mut library.borrow_mut(), outcomes);
                apply_add_report(
                    &ui,
                    &CarouselRefresh {
                        library: &library,
                        covers: &covers,
                        search: &search,
                        selection: &selection,
                        localizer: &localizer,
                    },
                    report,
                    op,
                    localizer.loader(),
                );
                // This generation's add is complete — hide the progress hairline.
                // (Superseded finalizes returned early above, so a newer in-flight
                // add's bar is left running for its own finalize to retire.)
                ui.set_add_active(false);
            })
        });
    }
}

#[allow(clippy::too_many_arguments)]
fn open_and_enter(
    ui: &ViewerWindow,
    nav: &Rc<RefCell<NavState>>,
    open_book: &Rc<use_cases::OpenBookUseCase>,
    state: &Rc<RefCell<ViewerState>>,
    viewport: &Rc<RefCell<ViewportState>>,
    pages: &Rc<PageController>,
    refresh: &CarouselRefresh<'_>,
    path: &std::path::Path,
) {
    // Writes back the OLD book first, opens the new path, then resumes its
    // stored page. Empty sources are removed instead of entering the viewer.
    let outcome = open_book.run(ui, path);
    // Enter the Viewer ONLY on a clean open: an empty source was already removed, and a
    // FAILED open must not drop the user into a 0-page Viewer (moved file / unmounted volume).
    let enter_viewer = matches!(outcome, use_cases::OpenOutcome::Success(..));
    let open_failed = matches!(outcome, use_cases::OpenOutcome::Error(_));
    finalize_open(ui, state, viewport, pages, refresh, outcome);
    // When the open failed because the file is gone/unmounted, replace the raw I/O status
    // with a book-named message. A failure with the file still present keeps the error.
    if open_failed && !path.exists() {
        let title = use_cases::book_display_title(&refresh.library.borrow(), path);
        ui.set_status_text(
            crate::i18n::dynamic::open_inaccessible(refresh.localizer.loader(), &title).into(),
        );
    }
    // Derive the title from authoritative post-open state so failed opens never
    // display the requested path as the current book.
    ui.set_current_book_name(current_book_name(state).into());
    if enter_viewer {
        go_to_viewer(ui, nav);
    }
}

/// Registers the library-search and carousel navigation/open callbacks onto `ui`.
/// Panel constraint (#151): explicit handle list IS the dependency list — no AppState bundle.
#[allow(clippy::too_many_arguments)]
pub(crate) fn wire_carousel_handlers(
    ui: &ViewerWindow,
    state: &Rc<RefCell<ViewerState>>,
    viewport: &Rc<RefCell<ViewportState>>,
    library: &Rc<RefCell<Library>>,
    nav: &Rc<RefCell<NavState>>,
    open_book: &Rc<use_cases::OpenBookUseCase>,
    covers: &Rc<cover_loader::CoverController>,
    pages: &Rc<PageController>,
    search: &Rc<RefCell<LibrarySearchState>>,
    selection: &Rc<RefCell<LibrarySelectionState>>,
    localizer: &Rc<i18n::Localizer>,
) {
    // Rebind the `&Rc<_>` params to owned `Rc` locals so each closure's `Rc::clone(&handle)`
    // prelude stays byte-identical to its pre-extraction form in `main`.
    let state = Rc::clone(state);
    let viewport = Rc::clone(viewport);
    let library = Rc::clone(library);
    let nav = Rc::clone(nav);
    let open_book = Rc::clone(open_book);
    let covers = Rc::clone(covers);
    let pages = Rc::clone(pages);
    let search = Rc::clone(search);
    let selection = Rc::clone(selection);
    let localizer = Rc::clone(localizer);

    // Library search: the debounced NavBar query (the ONLY query-update path). Replace the
    // filter, rebuild the filtered carousel + cover stream, reset focus to row 0.
    {
        let ui_weak = ui.as_weak();
        let library = Rc::clone(&library);
        let covers = Rc::clone(&covers);
        let search = Rc::clone(&search);
        let selection = Rc::clone(&selection);
        let localizer = Rc::clone(&localizer);
        ui.on_library_search_changed(move |query| {
            with_ui(&ui_weak, |ui| {
                // `search` and `library` are distinct RefCells, so a mut borrow of one and a
                // shared borrow of the other can't conflict; the shared borrow drops before refresh.
                search
                    .borrow_mut()
                    .set_query(query.to_string(), &library.borrow());
                // The selection (keyed by path) is ORTHOGONAL to the query: the rebuild
                // re-applies it over the new visible rows, so a query change never drops a book.
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
            })
        });
    }

    // Carousel open: Return on the focused book (or a double-click on the CENTER cover,
    // fired as the same `open(int)`); resumes its last-read page and enters the Viewer.
    {
        let ui_weak = ui.as_weak();
        let library = Rc::clone(&library);
        let nav = Rc::clone(&nav);
        let open_book = Rc::clone(&open_book);
        let state = Rc::clone(&state);
        let pages = Rc::clone(&pages);
        let search = Rc::clone(&search);
        let viewport = Rc::clone(&viewport);
        let localizer = Rc::clone(&localizer);
        // `finalize_open` may rebuild the carousel (empty-book auto-removal), so it
        // needs the full carousel-refresh deps.
        let covers = Rc::clone(&covers);
        let selection = Rc::clone(&selection);
        ui.on_carousel_open(move |index| {
            with_ui(&ui_weak, |ui| {
                // Resolve the VISIBLE carousel index to a Library path via the search
                // projection — the same hop others use, single-homed in `visible_index_to_path`.
                let Some(path) = visible_index_to_path(&library, &search, index) else {
                    // Index out of range (carousel and library out of sync) — no-op.
                    // Desync diagnostics (cold path): re-borrow is safe — the helper's borrows dropped.
                    let visible_len = search.borrow().visible_indices().len();
                    let library_len = library.borrow().books().len();
                    tracing::warn!(
                        index,
                        visible_len,
                        library_len,
                        "carousel-open: no book at index"
                    );
                    return;
                };
                open_and_enter(
                    &ui,
                    &nav,
                    &open_book,
                    &state,
                    &viewport,
                    &pages,
                    &CarouselRefresh {
                        library: &library,
                        covers: &covers,
                        search: &search,
                        selection: &selection,
                        localizer: &localizer,
                    },
                    &path,
                );
            })
        });
    }

    // Bookmark capsule: jump to the continue-reading book (`last_opened`). If still on
    // the shelf, open via the same path as Return; else (None/stale) show the no-bookmark notice.
    {
        let ui_weak = ui.as_weak();
        let library = Rc::clone(&library);
        let nav = Rc::clone(&nav);
        let open_book = Rc::clone(&open_book);
        let state = Rc::clone(&state);
        let viewport = Rc::clone(&viewport);
        let pages = Rc::clone(&pages);
        let localizer = Rc::clone(&localizer);
        // `finalize_open` may rebuild the carousel (empty-book auto-removal), so it
        // needs the full carousel-refresh deps.
        let covers = Rc::clone(&covers);
        let search = Rc::clone(&search);
        let selection = Rc::clone(&selection);
        ui.on_carousel_continue_reading(move || {
            with_ui(&ui_weak, |ui| {
                // `Library::bookmark()` returns `last_opened` only when still on the shelf;
                // a purged book yields None, counting as no bookmark (notice below).
                let path = library
                    .borrow()
                    .bookmark()
                    .map(std::path::Path::to_path_buf);
                let Some(path) = path else {
                    // No registered bookmark — answer the click with a notice so
                    // the always-enabled capsule still gives feedback.
                    ui.set_status_text(
                        crate::i18n::dynamic::bookmark_none(localizer.loader()).into(),
                    );
                    return;
                };
                open_and_enter(
                    &ui,
                    &nav,
                    &open_book,
                    &state,
                    &viewport,
                    &pages,
                    &CarouselRefresh {
                        library: &library,
                        covers: &covers,
                        search: &search,
                        selection: &selection,
                        localizer: &localizer,
                    },
                    &path,
                );
            })
        });
    }

    // Carousel move: Left/Right shift the focused cover by `delta`, clamped to shelf bounds
    // (hard stops, no wrap). focused-index is the single source of truth for the centered cover.
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
    // Carousel back: Down returns to the open book (Viewer). With no book open there is
    // nothing to return to (Viewer has no empty state), so refuse and explain via the strip.
    {
        let ui_weak = ui.as_weak();
        let nav = Rc::clone(&nav);
        let state = Rc::clone(&state);
        let localizer = Rc::clone(&localizer);
        ui.on_carousel_back(move || {
            with_ui(&ui_weak, |ui| {
                if state.borrow().open_file().is_none() {
                    ui.set_status_text(
                        crate::i18n::dynamic::no_open_book(localizer.loader()).into(),
                    );
                    return;
                }
                go_to_viewer(&ui, &nav);
            })
        });
    }
}

/// Registers the carousel selection/focus, bulk-delete, and empty-book-removal callbacks onto `ui`.
/// Panel constraint (#151): explicit handle list IS the dependency list — no AppState bundle.
#[allow(clippy::too_many_arguments)]
pub(crate) fn wire_selection_handlers(
    ui: &ViewerWindow,
    state: &Rc<RefCell<ViewerState>>,
    library: &Rc<RefCell<Library>>,
    covers: &Rc<cover_loader::CoverController>,
    search: &Rc<RefCell<LibrarySearchState>>,
    selection: &Rc<RefCell<LibrarySelectionState>>,
    localizer: &Rc<i18n::Localizer>,
) {
    // Rebind the `&Rc<_>` params to owned `Rc` locals so each closure's `Rc::clone(&handle)`
    // prelude stays byte-identical to its pre-extraction form in `main`.
    let state = Rc::clone(state);
    let library = Rc::clone(library);
    let covers = Rc::clone(covers);
    let search = Rc::clone(search);
    let selection = Rc::clone(selection);
    let localizer = Rc::clone(localizer);

    // Toggle the focused/clicked book's selection (VISIBLE index → path via the search
    // projection). Flips ONLY that row's `selected` flag, no model rebuild; desync = no-op.
    {
        let ui_weak = ui.as_weak();
        let library = Rc::clone(&library);
        let search = Rc::clone(&search);
        let selection = Rc::clone(&selection);
        let localizer = Rc::clone(&localizer);
        ui.on_carousel_toggle_selection(move |index| {
            with_ui(&ui_weak, |ui| {
                let Some(path) = visible_index_to_path(&library, &search, index) else {
                    // Desync diagnostics (cold path): re-borrow is safe — the helper's borrows dropped.
                    let visible_len = search.borrow().visible_indices().len();
                    let library_len = library.borrow().books().len();
                    tracing::warn!(
                        index,
                        visible_len,
                        library_len,
                        "carousel-toggle-selection: no book at index"
                    );
                    return;
                };
                selection.borrow_mut().toggle(path.clone());
                let selected = selection.borrow().contains(&path);
                set_carousel_selected(&ui, index as usize, selected);
                push_selection_toolbar_state(&ui, &localizer, &selection, &search, &library);
            })
        });
    }

    // Cover clicked. NORMAL mode: only the CENTER cover arrives (side clicks fire move(∓1)),
    // and it only FOCUSES (open = Return/double-click). SELECTION mode: any click focuses + toggles.
    {
        let ui_weak = ui.as_weak();
        let library = Rc::clone(&library);
        let search = Rc::clone(&search);
        let selection = Rc::clone(&selection);
        let localizer = Rc::clone(&localizer);
        ui.on_carousel_cover_clicked(move |index| {
            with_ui(&ui_weak, |ui| {
                // Always focus the clicked cover (carousel and click both drive
                // focused-index). The visible index IS the carousel row index.
                ui.set_carousel_focused_index(index);
                if !ui.get_carousel_selection_mode() {
                    return; // normal mode: focus only, never open
                }
                let Some(path) = visible_index_to_path(&library, &search, index) else {
                    // Desync diagnostics (cold path): re-borrow is safe — the helper's borrows dropped.
                    let visible_len = search.borrow().visible_indices().len();
                    let library_len = library.borrow().books().len();
                    tracing::warn!(
                        index,
                        visible_len,
                        library_len,
                        "carousel-cover-clicked: no book at index in selection mode"
                    );
                    return;
                };
                selection.borrow_mut().toggle(path.clone());
                let selected = selection.borrow().contains(&path);
                set_carousel_selected(&ui, index as usize, selected);
                push_selection_toolbar_state(&ui, &localizer, &selection, &search, &library);
            })
        });
    }

    // Select-all / deselect-all toggle (toolbar button or Cmd/Ctrl+A): if all visible are
    // selected, deselect all; else select all. Re-applies flags + refreshes toolbar strings.
    {
        let ui_weak = ui.as_weak();
        let library = Rc::clone(&library);
        let search = Rc::clone(&search);
        let selection = Rc::clone(&selection);
        let localizer = Rc::clone(&localizer);
        ui.on_carousel_select_all(move || {
            with_ui(&ui_weak, |ui| {
                {
                    let lib = library.borrow();
                    let srch = search.borrow();
                    let mut sel = selection.borrow_mut();
                    if selection_projection::all_visible_selected(&sel, &srch, &lib) {
                        selection_projection::deselect_visible(&mut sel, &srch, &lib);
                    } else {
                        selection_projection::select_visible(&mut sel, &srch, &lib);
                    }
                }
                // Re-apply the (updated) selection flags over the visible rows so
                // every badge appears/disappears without a full carousel rebuild.
                {
                    let lib = library.borrow();
                    let indices = search.borrow().visible_indices().to_vec();
                    let sel = selection.borrow();
                    apply_selection_flags(&ui, &lib, &indices, |path| sel.contains(path));
                }
                push_selection_toolbar_state(&ui, &localizer, &selection, &search, &library);
            })
        });
    }

    // Leave selection mode (Esc / toolbar exit). Slint already cleared `selection-mode`;
    // clear the Rust selection set and re-apply the now-empty flags so every badge disappears.
    {
        let ui_weak = ui.as_weak();
        let library = Rc::clone(&library);
        let search = Rc::clone(&search);
        let selection = Rc::clone(&selection);
        let localizer = Rc::clone(&localizer);
        ui.on_carousel_exit_selection(move || {
            with_ui(&ui_weak, |ui| {
                selection.borrow_mut().clear();
                let lib = library.borrow();
                let indices = search.borrow().visible_indices().to_vec();
                apply_selection_flags(&ui, &lib, &indices, |_| false);
                push_selection_toolbar_state(&ui, &localizer, &selection, &search, &library);
            })
        });
    }

    // Delete requested (toolbar or Delete/Backspace). Fired even at N=0, so an empty
    // selection is a no-op; else build the confirm-dialog content and mount the modal.
    {
        let ui_weak = ui.as_weak();
        let state = Rc::clone(&state);
        let library = Rc::clone(&library);
        let search = Rc::clone(&search);
        let selection = Rc::clone(&selection);
        let localizer = Rc::clone(&localizer);
        ui.on_carousel_request_delete(move || {
            with_ui(&ui_weak, |ui| {
                // No-op on an empty selection (the key arm accepts even at zero).
                if selection.borrow().count() == 0 {
                    return;
                }
                // Build the dialog content under one shared-borrow group: the four distinct
                // RefCells hold immutable Refs safely; the owned result drops before UI setters.
                let content = {
                    let st = state.borrow();
                    use_cases::confirm_delete_content(
                        localizer.loader(),
                        &selection.borrow(),
                        &search.borrow(),
                        &library.borrow(),
                        st.open_file(),
                    )
                };
                ui.set_confirm_delete_title(content.title.into());
                // `confirm-delete-body-lines` is a Slint `[string]`, so its setter takes a
                // `ModelRc<SharedString>`; wrap the lines in a one-shot `VecModel`.
                let body_lines: Vec<slint::SharedString> =
                    content.body_lines.into_iter().map(Into::into).collect();
                ui.set_confirm_delete_body_lines(slint::ModelRc::new(slint::VecModel::from(
                    body_lines,
                )));
                ui.set_confirm_delete_info(content.info.into());
                ui.set_confirm_delete_warning(content.warning.into());
                ui.set_show_confirm_delete(true);
            })
        });
    }

    // Delete confirmed: run the `RemoveBooksUseCase` transaction (mutate + save with
    // rollback, cover purge, close-if-open), then finalize the UI. Modal dismissed always.
    {
        let ui_weak = ui.as_weak();
        let library = Rc::clone(&library);
        let search = Rc::clone(&search);
        let selection = Rc::clone(&selection);
        let localizer = Rc::clone(&localizer);
        let covers = Rc::clone(&covers);
        let remove_books = use_cases::RemoveBooksUseCase::new(
            Rc::clone(&state),
            Rc::clone(&library),
            Rc::clone(&search),
            Rc::clone(&selection),
        );
        ui.on_confirm_delete_accepted(move || {
            with_ui(&ui_weak, |ui| {
                let outcome = remove_books.run();
                // Dismiss the modal in every outcome (its stale content props are
                // rebuilt on the next open).
                ui.set_show_confirm_delete(false);
                finalize_remove(
                    &ui,
                    &CarouselRefresh {
                        library: &library,
                        covers: &covers,
                        search: &search,
                        selection: &selection,
                        localizer: &localizer,
                    },
                    outcome,
                );
            })
        });
    }

    // A cover worker found a book with zero image pages: auto-remove it, persist, purge
    // its cover, rebuild the carousel, and notice. Idempotent (second signal = no-op).
    {
        let ui_weak = ui.as_weak();
        let library = Rc::clone(&library);
        let covers = Rc::clone(&covers);
        let search = Rc::clone(&search);
        let selection = Rc::clone(&selection);
        let localizer = Rc::clone(&localizer);
        ui.on_empty_book_detected(move |path_str| {
            with_ui(&ui_weak, |ui| {
                let path = std::path::PathBuf::from(path_str.as_str());
                // The shared transaction (single home in open_book): title capture
                // BEFORE removal -> Library::remove -> save -> best-effort cover purge.
                let removal = use_cases::remove_empty_book(&library, &path);
                finalize_empty_book_removed(
                    &ui,
                    &CarouselRefresh {
                        library: &library,
                        covers: &covers,
                        search: &search,
                        selection: &selection,
                        localizer: &localizer,
                    },
                    &removal,
                );
            })
        });
    }
}
