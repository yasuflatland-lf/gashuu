use crate::library_model::{LibrarySearchState, LibrarySelectionState};
use crate::navigation::NavState;
use crate::viewer_state::ViewerState;
use crate::viewport::ViewportState;
use crate::{
    add_books_and_refresh, current_book_name, finalize_open, go_to_viewer,
    refresh_library_carousel, with_ui, CarouselRefresh, ViewerWindow,
};
use crate::{app, cover_loader, i18n};
use app::SkippedDetail;
use gashuu_core::Library;
use slint::ComponentHandle;
use std::cell::RefCell;
use std::rc::Rc;

// Panel constraint (#151): no AppState bundle — the explicit handle list IS the dependency list.
#[allow(clippy::too_many_arguments)]
pub(crate) fn wire_open_handlers(
    ui: &ViewerWindow,
    open_book: &Rc<app::OpenBookUseCase>,
    state: &Rc<RefCell<ViewerState>>,
    viewport: &Rc<RefCell<ViewportState>>,
    library: &Rc<RefCell<Library>>,
    covers: &Rc<cover_loader::CoverController>,
    search: &Rc<RefCell<LibrarySearchState>>,
    selection: &Rc<RefCell<LibrarySelectionState>>,
    localizer: &Rc<i18n::Localizer>,
) {
    // Rebind the `&Rc<_>` parameters to owned `Rc` locals so each closure's
    // `Rc::clone(&handle)` prelude stays byte-identical to its pre-extraction
    // form in `main` (cloning an owned `Rc`, not a `&Rc`).
    let open_book = Rc::clone(open_book);
    let state = Rc::clone(state);
    let viewport = Rc::clone(viewport);
    let library = Rc::clone(library);
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
    library: &Rc<RefCell<Library>>,
    nav: &Rc<RefCell<NavState>>,
    open_book: &Rc<app::OpenBookUseCase>,
    state: &Rc<RefCell<ViewerState>>,
    viewport: &Rc<RefCell<ViewportState>>,
    covers: &Rc<cover_loader::CoverController>,
    search: &Rc<RefCell<LibrarySearchState>>,
    selection: &Rc<RefCell<LibrarySelectionState>>,
    localizer: &Rc<i18n::Localizer>,
) {
    // Rebind the `&Rc<_>` parameters to owned `Rc` locals so each closure's
    // `Rc::clone(&handle)` prelude stays byte-identical to its pre-extraction
    // form in `main` (cloning an owned `Rc`, not a `&Rc`).
    let library = Rc::clone(library);
    let nav = Rc::clone(nav);
    let open_book = Rc::clone(open_book);
    let state = Rc::clone(state);
    let viewport = Rc::clone(viewport);
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

    // Carousel: Return on the focused book opens it, resumes its last-read
    // page (via OpenBookUseCase::run → jump_to), and transitions to the Viewer.
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
