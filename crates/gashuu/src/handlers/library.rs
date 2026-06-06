use crate::{
    add_books_and_refresh, clamp_focused_index, current_book_name, empty_book_removed_status,
    finalize_open, go_to_viewer, push_selection_strings, refresh_library_carousel,
    visible_index_to_path, with_ui, CarouselRefresh, ViewerWindow,
};
use crate::{
    app,
    carousel::{apply_selection_flags, set_carousel_selected},
    cover_loader, i18n,
};
use crate::{
    library_model::{LibrarySearchState, LibrarySelectionState},
    navigation::NavState,
};
use crate::{viewer_state::ViewerState, viewport::ViewportState};
use app::SkippedDetail;
use gashuu_core::Library;
use slint::ComponentHandle;
use std::cell::RefCell;
use std::rc::Rc;

/// Registers the open-folder/open-archive and add-books/add-folder callbacks onto `ui`.
/// Panel constraint (#151): explicit handle list IS the dependency list — no AppState bundle.
#[allow(clippy::too_many_arguments)]
pub(crate) fn wire_open_handlers(
    ui: &ViewerWindow,
    state: &Rc<RefCell<ViewerState>>,
    viewport: &Rc<RefCell<ViewportState>>,
    library: &Rc<RefCell<Library>>,
    open_book: &Rc<app::OpenBookUseCase>,
    covers: &Rc<cover_loader::CoverController>,
    search: &Rc<RefCell<LibrarySearchState>>,
    selection: &Rc<RefCell<LibrarySelectionState>>,
    localizer: &Rc<i18n::Localizer>,
) {
    // Rebind the `&Rc<_>` parameters to owned `Rc` locals so each closure's
    // `Rc::clone(&handle)` prelude stays byte-identical to its pre-extraction
    // form in `main` (cloning an owned `Rc`, not a `&Rc`).
    let state = Rc::clone(state);
    let viewport = Rc::clone(viewport);
    let library = Rc::clone(library);
    let open_book = Rc::clone(open_book);
    let covers = Rc::clone(covers);
    let search = Rc::clone(search);
    let selection = Rc::clone(selection);
    let localizer = Rc::clone(localizer);

    // Open Folder button: pick a directory, open it, refresh the view, and start thumbnail generation.
    {
        let ui_weak = ui.as_weak();
        let open_book = Rc::clone(&open_book);
        let state = Rc::clone(&state);
        let viewport = Rc::clone(&viewport);
        let localizer = Rc::clone(&localizer);
        // `finalize_open` may rebuild the carousel (empty-book auto-removal), so it
        // needs the full carousel-refresh deps, not just the localizer.
        let library = Rc::clone(&library);
        let covers = Rc::clone(&covers);
        let search = Rc::clone(&search);
        let selection = Rc::clone(&selection);
        ui.on_open_folder(move || {
            with_ui(&ui_weak, |ui| {
                let Some(dir) = rfd::FileDialog::new().pick_folder() else {
                    return;
                };
                let outcome = open_book.run(&ui, &dir, SkippedDetail::None);
                finalize_open(
                    &ui,
                    &state,
                    &viewport,
                    &CarouselRefresh {
                        library: &library,
                        covers: &covers,
                        search: &search,
                        selection: &selection,
                        localizer: &localizer,
                    },
                    outcome,
                );
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
        let viewport = Rc::clone(&viewport);
        let localizer = Rc::clone(&localizer);
        // `finalize_open` may rebuild the carousel (empty-book auto-removal), so it
        // needs the full carousel-refresh deps, not just the localizer.
        let library = Rc::clone(&library);
        let covers = Rc::clone(&covers);
        let search = Rc::clone(&search);
        let selection = Rc::clone(&selection);
        ui.on_open_archive(move || {
            with_ui(&ui_weak, |ui| {
                let Some(file) = rfd::FileDialog::new()
                    .add_filter("Comic archive", &["cbz", "zip", "cbr", "rar"])
                    .pick_file()
                else {
                    return;
                };
                let outcome = open_book.run(&ui, &file, SkippedDetail::Archive);
                finalize_open(
                    &ui,
                    &state,
                    &viewport,
                    &CarouselRefresh {
                        library: &library,
                        covers: &covers,
                        search: &search,
                        selection: &selection,
                        localizer: &localizer,
                    },
                    outcome,
                );
                // Title-bar book name is derived from the AUTHORITATIVE post-open
                // state (the canonical `open_file`), so a FAILED open (corrupt /
                // non-archive file) never shows the picked file's name: on failure
                // `open_file` is unchanged and `run` already set an `Error:` status.
                ui.set_current_book_name(current_book_name(&state).into());
            })
        });
    }

    // Add Books button: pick comic sources and add them to the library. On
    // macOS a single NSOpenPanel picks archives AND folders together
    // (`pick_files_or_folders` only compiles there); elsewhere this is the
    // files-only picker paired with the separate Add Folder button below. Rust
    // is the single authority for the dialog flavor — Slint only fires the
    // intent. Skips duplicates and rejects image-free or unreadable sources
    // (via `add_paths`), persists, rebuilds the carousel model, and restores
    // keyboard focus to the carousel.
    {
        let ui_weak = ui.as_weak();
        let library = Rc::clone(&library);
        let covers = Rc::clone(&covers);
        let search = Rc::clone(&search);
        let selection = Rc::clone(&selection);
        let localizer = Rc::clone(&localizer);
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
                add_books_and_refresh(
                    &ui,
                    &CarouselRefresh {
                        library: &library,
                        covers: &covers,
                        search: &search,
                        selection: &selection,
                        localizer: &localizer,
                    },
                    paths,
                    "add-books",
                    localizer.loader(),
                );
            })
        });
    }

    // Add Folder button: pick a single folder and add it as one book to the
    // library. Wraps the folder in a `vec![]` so the same dedup/save/rebuild
    // path as `on_add_books` is used. Skips duplicates and rejects image-free
    // or unreadable sources (via `add_paths`), persists, and restores carousel
    // focus.
    {
        let ui_weak = ui.as_weak();
        let library = Rc::clone(&library);
        let covers = Rc::clone(&covers);
        let search = Rc::clone(&search);
        let selection = Rc::clone(&selection);
        let localizer = Rc::clone(&localizer);
        ui.on_add_folder(move || {
            with_ui(&ui_weak, |ui| {
                let Some(folder) = rfd::FileDialog::new().pick_folder() else {
                    return;
                };
                add_books_and_refresh(
                    &ui,
                    &CarouselRefresh {
                        library: &library,
                        covers: &covers,
                        search: &search,
                        selection: &selection,
                        localizer: &localizer,
                    },
                    vec![folder],
                    "add-folder",
                    localizer.loader(),
                );
            })
        });
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
    open_book: &Rc<app::OpenBookUseCase>,
    covers: &Rc<cover_loader::CoverController>,
    search: &Rc<RefCell<LibrarySearchState>>,
    selection: &Rc<RefCell<LibrarySelectionState>>,
    localizer: &Rc<i18n::Localizer>,
) {
    // Rebind the `&Rc<_>` parameters to owned `Rc` locals so each closure's
    // `Rc::clone(&handle)` prelude stays byte-identical to its pre-extraction
    // form in `main` (cloning an owned `Rc`, not a `&Rc`).
    let state = Rc::clone(state);
    let viewport = Rc::clone(viewport);
    let library = Rc::clone(library);
    let nav = Rc::clone(nav);
    let open_book = Rc::clone(open_book);
    let covers = Rc::clone(covers);
    let search = Rc::clone(search);
    let selection = Rc::clone(selection);
    let localizer = Rc::clone(localizer);

    // Library search: the debounced query from the NavBar search field. This is
    // the ONLY query-update path from Slint. Replace the filter (clearing any
    // forced-visible just-added books), then rebuild the filtered carousel + cover
    // stream and reset focus to row 0 so the first match is centered.
    {
        let ui_weak = ui.as_weak();
        let library = Rc::clone(&library);
        let covers = Rc::clone(&covers);
        let search = Rc::clone(&search);
        let selection = Rc::clone(&selection);
        let localizer = Rc::clone(&localizer);
        ui.on_library_search_changed(move |query| {
            with_ui(&ui_weak, |ui| {
                // `search` and `library` are distinct RefCells; borrowing one
                // mut while the other is shared cannot conflict. The shared
                // `library.borrow()` drops at the `;` before refresh.
                search
                    .borrow_mut()
                    .set_query(query.to_string(), &library.borrow());
                // The selection (keyed by path) is ORTHOGONAL to the query: the
                // rebuild re-applies the selection over the new visible rows, so a
                // query change never drops a selected book.
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

    // Carousel: Return on the focused book — or a double-click on the CENTER
    // strip cover (the Slint side fires the same `open(int)` with that cover's
    // index; normal mode only — side clicks are intercepted by the one-step
    // zones) — opens it, resumes its last-read page (via
    // OpenBookUseCase::run → jump_to), and transitions to the Viewer.
    {
        let ui_weak = ui.as_weak();
        let library = Rc::clone(&library);
        let nav = Rc::clone(&nav);
        let open_book = Rc::clone(&open_book);
        let state = Rc::clone(&state);
        let search = Rc::clone(&search);
        let viewport = Rc::clone(&viewport);
        let localizer = Rc::clone(&localizer);
        // `finalize_open` may rebuild the carousel (empty-book auto-removal), so it
        // needs the full carousel-refresh deps.
        let covers = Rc::clone(&covers);
        let selection = Rc::clone(&selection);
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
                let outcome = open_book.run(&ui, &path, SkippedDetail::None);
                // An empty source is removed instead of opened: stay on the
                // Library (the rebuilt carousel no longer shows it) rather than
                // switching to an empty viewer.
                let enter_viewer = !matches!(outcome, app::OpenOutcome::EmptyBookRemoved { .. });
                finalize_open(
                    &ui,
                    &state,
                    &viewport,
                    &CarouselRefresh {
                        library: &library,
                        covers: &covers,
                        search: &search,
                        selection: &selection,
                        localizer: &localizer,
                    },
                    outcome,
                );
                // Title-bar book name is derived from the AUTHORITATIVE post-open
                // state (the canonical `open_file`), so a FAILED open (a Library
                // book that was moved/deleted) never shows that book's name: on
                // failure `open_file` is unchanged and `run` already set an
                // `Error:` status. Enter the viewer only when the open actually
                // produced a book to show — an empty source was auto-removed and
                // `finalize_open` left us on a refreshed Library.
                ui.set_current_book_name(current_book_name(&state).into());
                if enter_viewer {
                    go_to_viewer(&ui, &nav);
                }
            })
        });
    }

    // NavBar bookmark capsule: jump to the continue-reading book. The bookmark IS
    // the library's `last_opened` book (the same path the continue-reading ribbon
    // marks). When it names a book still present in the library, open it through
    // the SAME path a Return-on-cover open uses — open_book.run resumes its stored
    // page for free — then transition to the Viewer. When there is no bookmark
    // (None) OR it names a book no longer in the library (a stale path purged from
    // `books`), there is nothing to jump to: surface the no-bookmark notice instead.
    {
        let ui_weak = ui.as_weak();
        let library = Rc::clone(&library);
        let nav = Rc::clone(&nav);
        let open_book = Rc::clone(&open_book);
        let state = Rc::clone(&state);
        let viewport = Rc::clone(&viewport);
        let localizer = Rc::clone(&localizer);
        // `finalize_open` may rebuild the carousel (empty-book auto-removal), so it
        // needs the full carousel-refresh deps.
        let covers = Rc::clone(&covers);
        let search = Rc::clone(&search);
        let selection = Rc::clone(&selection);
        ui.on_carousel_continue_reading(move || {
            with_ui(&ui_weak, |ui| {
                // `Library::bookmark()` returns `last_opened` only when it is
                // still on the shelf — books purged from the library yield None,
                // which counts as no bookmark and triggers the notice below.
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
                // Same open sequence as on_carousel_open: open_book.run writes back
                // the OLD book's position, opens the bookmarked path, and resumes
                // its stored page; a failed open (file moved/deleted) is handled by
                // run itself (leaves open_file unchanged + sets an `Error:` status).
                let outcome = open_book.run(&ui, &path, SkippedDetail::None);
                // An empty source is removed instead of opened: stay on the
                // Library (the rebuilt carousel no longer shows it) rather than
                // switching to an empty viewer.
                let enter_viewer = !matches!(outcome, app::OpenOutcome::EmptyBookRemoved { .. });
                finalize_open(
                    &ui,
                    &state,
                    &viewport,
                    &CarouselRefresh {
                        library: &library,
                        covers: &covers,
                        search: &search,
                        selection: &selection,
                        localizer: &localizer,
                    },
                    outcome,
                );
                ui.set_current_book_name(current_book_name(&state).into());
                if enter_viewer {
                    go_to_viewer(&ui, &nav);
                }
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
    // Carousel: Down returns to the currently-open book (the Viewer). With no
    // book open there is nothing to return to — the Viewer would render an
    // all-black stage (it has no empty-state chrome of its own) — so the
    // navigation is refused and the Library status strip explains why instead.
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
    // Rebind the `&Rc<_>` parameters to owned `Rc` locals so each closure's
    // `Rc::clone(&handle)` prelude stays byte-identical to its pre-extraction
    // form in `main` (cloning an owned `Rc`, not a `&Rc`).
    let state = Rc::clone(state);
    let library = Rc::clone(library);
    let covers = Rc::clone(covers);
    let search = Rc::clone(search);
    let selection = Rc::clone(selection);
    let localizer = Rc::clone(localizer);

    // Carousel: toggle the focused/clicked book's bulk-selection membership
    // (keyboard `x`/Space, forwarded as a VISIBLE carousel index). Resolves the
    // visible index → library path through the search projection (the SAME hop as
    // `on_carousel_open`), toggles the path in the selection set, then flips ONLY
    // that row's `selected` flag so its accent badge appears/disappears without a
    // model rebuild. Out-of-range / desync indices are a no-op (warn on desync).
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
                push_selection_strings(&ui, &localizer, &selection, &search, &library);
            })
        });
    }

    // Carousel: a cover was clicked (the repo's first cover pointer interaction).
    // In NORMAL mode only the CENTER-strip cover can arrive here — the Slint
    // left/right step zones intercept every side click and fire `move(∓1)`
    // instead (one book per click) — and the click only FOCUSES it; opening is
    // Return or a center-cover DOUBLE-click (both arrive via `on_carousel_open`
    // — the Slint side fires `open(int)` from its double-clicked arm, so no
    // second open path exists here). In SELECTION mode the zones are disabled,
    // so ANY cover click lands here and focuses AND toggles that book.
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
                push_selection_strings(&ui, &localizer, &selection, &search, &library);
            })
        });
    }

    // Carousel: select-all / deselect-all toggle. Routed here by both the toolbar
    // button and Cmd/Ctrl+A from the Slint side. If every visible book is already
    // selected, this deselects them all (via `deselect_visible`); otherwise it
    // selects them all (via `select_visible`). Re-applies the selection flags over
    // the visible rows and refreshes the toolbar strings.
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
                    if sel.all_visible_selected(&srch, &lib) {
                        sel.deselect_visible(&srch, &lib);
                    } else {
                        sel.select_visible(&srch, &lib);
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
                push_selection_strings(&ui, &localizer, &selection, &search, &library);
            })
        });
    }

    // Carousel: leave selection mode (Esc or toolbar exit button). The Slint
    // caller (Esc key arm or toolbar exit button) already cleared `selection-mode`;
    // here we clear the Rust selection set and re-apply the (now empty) flags over
    // the visible rows so every badge disappears. A fresh re-entry into selection
    // mode then starts with nothing selected.
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
                push_selection_strings(&ui, &localizer, &selection, &search, &library);
            })
        });
    }

    // Carousel: a delete was requested (toolbar DangerButton or the Delete /
    // Backspace key arm). The Slint side fires this even at N=0 (the key arm is
    // unconditional by design), so an empty selection is a no-op here — the
    // confirm dialog is never shown for nothing. Otherwise, build the localized
    // confirm-dialog content for the current selection and push it into the
    // ConfirmDialog's in-out properties, then flip `show-confirm-delete` true to
    // mount the modal. Cancel/Esc/backdrop are handled purely in Slint (selection
    // PRESERVED); Rust only sees the accept (the handler below).
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
                // Build the dialog content under one shared-borrow group:
                // `state`, `selection`, `search`, and `library` are distinct
                // `RefCell`s, so holding the four immutable `Ref`s together is
                // safe; the `ConfirmDeleteContent` it returns owns its strings, so
                // the group drops at the block's `}` before the UI setters run.
                let content = {
                    let st = state.borrow();
                    app::confirm_delete_content(
                        localizer.loader(),
                        &selection.borrow(),
                        &search.borrow(),
                        &library.borrow(),
                        st.open_file(),
                    )
                };
                ui.set_confirm_delete_title(content.title.into());
                // `confirm-delete-body-lines` is a Slint `[string]` property, so its
                // setter takes a `ModelRc<SharedString>`; wrap the owned lines in a
                // one-shot `VecModel` (mirrors `carousel::model`'s `ModelRc::new`).
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

    // Carousel: the delete confirmation was accepted (ConfirmDialog primary
    // action). Run the destructive `RemoveBooksUseCase` transaction (mutate +
    // save with rollback, cover purge, viewer-close-if-open, search recompute,
    // selection clear), then finalize the UI from the returned `RemoveOutcome`.
    // The modal is dismissed in EVERY outcome (its stale content props are
    // rebuilt on the next open). The use case is constructed once and moved into
    // the closure (mirrors how `OpenBookUseCase` is held).
    {
        let ui_weak = ui.as_weak();
        let library = Rc::clone(&library);
        let search = Rc::clone(&search);
        let selection = Rc::clone(&selection);
        let localizer = Rc::clone(&localizer);
        let covers = Rc::clone(&covers);
        let remove_books = app::RemoveBooksUseCase::new(
            Rc::clone(&state),
            Rc::clone(&library),
            Rc::clone(&search),
            Rc::clone(&selection),
        );
        ui.on_confirm_delete_accepted(move || {
            with_ui(&ui_weak, |ui| {
                let loader = localizer.loader();
                // The use case owns the destructive transaction in the issue's
                // non-negotiable order, including the viewer-close (it cleared
                // `current_book_name` itself) and the success-only selection clear.
                let outcome = remove_books.run(&ui);
                // Dismiss the modal in every outcome.
                ui.set_show_confirm_delete(false);
                match outcome {
                    app::RemoveOutcome::NoSelection => {
                        // Defensive: the request handler guards against an empty
                        // selection, so this should not be reached. Just refocus.
                        ui.invoke_focus_carousel();
                    }
                    app::RemoveOutcome::SaveFailed { error } => {
                        // The shelf was rolled back and the selection PRESERVED by
                        // `run` (no flag re-apply needed — the rows are unchanged).
                        // Stay in selection mode so the user can retry.
                        ui.set_status_text(
                            crate::i18n::dynamic::delete_save_failed(loader, &error).into(),
                        );
                        ui.invoke_focus_carousel();
                    }
                    app::RemoveOutcome::Removed { n, .. } => {
                        // `run` already recomputed the search projection and cleared
                        // the selection. Rebuild the carousel from the fresh visible
                        // set (no focus reset — we clamp the focused index below to a
                        // valid row), so the rebuilt model reflects the shrunken shelf.
                        refresh_library_carousel(
                            &ui,
                            &CarouselRefresh {
                                library: &library,
                                covers: &covers,
                                search: &search,
                                selection: &selection,
                                localizer: &localizer,
                            },
                            false,
                        );
                        // Clamp the focused index into the NEW visible row count
                        // BEFORE the Slint side can read a stale out-of-range value.
                        // The model is already bound above; setting the focused index
                        // now re-centers the carousel on a valid row (index-out-of-
                        // range on the projection is the documented crash risk).
                        let visible_count = search.borrow().visible_indices().len();
                        let clamped =
                            clamp_focused_index(ui.get_carousel_focused_index(), visible_count);
                        ui.set_carousel_focused_index(clamped);
                        // Exit selection mode: drop the toolbar and clear every row's
                        // `selected` flag (selection itself was already cleared by
                        // `run`, so do NOT double-clear it — just re-apply all-false).
                        ui.set_carousel_selection_mode(false);
                        {
                            let lib = library.borrow();
                            let indices = search.borrow().visible_indices().to_vec();
                            apply_selection_flags(&ui, &lib, &indices, |_| false);
                        }
                        push_selection_strings(&ui, &localizer, &selection, &search, &library);
                        // Status push AFTER the refresh + toolbar string updates (the
                        // same status-last ordering `add_books_and_refresh` uses), so
                        // the deleted-books notice is the final write to the Library's
                        // bottom strip. `n` already excludes stale not_found paths.
                        ui.set_status_text(crate::i18n::dynamic::deleted_books(loader, n).into());
                        // Restore keyboard focus to the carousel so its key seams work.
                        // `run` cleared the viewer + `current_book_name` itself when the
                        // open book was deleted; `current_book_name` is derived on demand
                        // from `state.open_file()` (now `None`), so no main.rs mirror
                        // state needs syncing here.
                        ui.invoke_focus_carousel();
                    }
                }
            })
        });
    }

    // Carousel: a cover-loading worker found a book whose source has zero image
    // pages (an empty folder, an archive emptied since it was added, …). The
    // worker invokes this with the book's canonical path (epoch-guarded on its
    // side so a stale in-flight result is dropped). Auto-remove the now-empty
    // book from the library, persist, purge its cached cover, rebuild the
    // carousel (the rebuild's epoch bump drops any sibling covers still
    // streaming for it), and surface a notice. Idempotent: a second signal for a
    // book already removed (`Library::remove` returns false) is a silent no-op.
    {
        let ui_weak = ui.as_weak();
        let library = Rc::clone(&library);
        let covers = Rc::clone(&covers);
        let search = Rc::clone(&search);
        let selection = Rc::clone(&selection);
        let localizer = Rc::clone(&localizer);
        ui.on_empty_book_detected(move |path_str| {
            with_ui(&ui_weak, |ui| {
                let loader = localizer.loader();
                let path = std::path::PathBuf::from(path_str.as_str());
                // The shared transaction (single home in app.rs): title capture
                // BEFORE removal → `Library::remove` → save → best-effort cover
                // purge. `removed == false` is the idempotency race — the book
                // was already removed by another path (its notice + rebuild
                // already ran), so bail out silently.
                let removal = app::remove_empty_book(&library, &path);
                if !removal.removed {
                    return;
                }
                // Rebuild the carousel so the removed book disappears and the
                // cover-epoch bump drops any sibling cover still streaming for it;
                // the active search filter is preserved by the chokepoint. No focus
                // reset — the user's focus stays where it was.
                refresh_library_carousel(
                    &ui,
                    &CarouselRefresh {
                        library: &library,
                        covers: &covers,
                        search: &search,
                        selection: &selection,
                        localizer: &localizer,
                    },
                    false,
                );
                // Notice LAST (status-last ordering, as in add/delete): the
                // auto-removal message, with the save-failure detail appended when
                // the persist failed.
                let status = empty_book_removed_status(
                    loader,
                    &removal.title,
                    removal.save_error.as_deref(),
                );
                ui.set_status_text(status.into());
            })
        });
    }
}
