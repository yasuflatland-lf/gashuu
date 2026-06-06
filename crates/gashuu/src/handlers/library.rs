use crate::library_model::{LibrarySearchState, LibrarySelectionState};
use crate::viewer_state::ViewerState;
use crate::viewport::ViewportState;
use crate::{
    add_books_and_refresh, current_book_name, finalize_open, with_ui, CarouselRefresh, ViewerWindow,
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
