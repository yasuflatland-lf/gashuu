slint::include_modules!();

mod app;
mod carousel;
mod cover_loader;
mod enum_adapters;
mod i18n;
mod keymap;
mod library_model;
mod navigation;
mod page_jump;
mod thumbnail_strip;
mod viewer_state;
mod viewport;

use app::SkippedDetail;
use carousel::{
    apply_selection_flags, bind_carousel_model, build_carousel_model, cover_requests,
    set_carousel_selected, thumb_state_at, ThumbState,
};
use enum_adapters::{
    cover_mode_to_index, fit_mode_to_index, index_to_cover_mode, index_to_fit_mode,
    index_to_language, index_to_reading_direction, index_to_spread_mode, language_to_index,
    reading_direction_to_index, spread_mode_to_index,
};
use gashuu_core::{
    ArchiveLoader, CacheConfig, CoreError, DecodedImage, FitMode, Library, ReadingDirection,
    Settings, ViewOverride,
};
use keymap::{map_key, KeyCommand};
use library_model::{LibrarySearchState, LibrarySelectionState};
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
    // Boot the Fluent localizer with the persisted language; `apply()` pushes
    // every static string into the Strings global before the first paint.
    let localizer = Rc::new(i18n::Localizer::new(settings.language));
    localizer.apply(&ui);
    // Platform capability, pushed once: macOS' NSOpenPanel picks files AND
    // folders in one panel, so the Library NavBar collapses its two add
    // capsules into a single combined one (the dialog flavor itself is decided
    // in `on_add_books`). Compile-time constant — never changes at runtime.
    ui.set_combined_add_picker(cfg!(target_os = "macos"));
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

    // Shared bulk-selection state (bulk-delete epic, PR-2). Owned here and shared
    // via `Rc` with the carousel toggle / cover-click / exit handlers and the
    // carousel refresh (which re-applies the selection flags over the rebuilt
    // visible rows). Keyed by path, so it is orthogonal to the search projection —
    // a query change never drops a selection. Nothing is deleted in this PR.
    let selection = Rc::new(RefCell::new(LibrarySelectionState::default()));

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
    // Continue reading: the app boots on the Library screen, so override the
    // refresh's reset-to-0 with a one-shot snap to the last-read book's visible
    // row, resolved through the empty-query visible set seeded above.
    snap_carousel_focus_to_last_opened(&ui, &library, &search);

    // One startup sweep keeps the cover cache under its size cap and reclaims
    // key-orphaned covers (issue 143). Dispatched AFTER the initial cover
    // stream above so the visible covers grab the rayon workers first.
    cover_loader::spawn_cache_prune();

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
    refresh(&ui, &state.borrow(), &viewport, localizer.loader());

    // Surface any load failures AFTER the initial refresh, which overwrites
    // status-text with "No folder opened". The carousel/home screen is visible
    // at this point, so the user sees the notice immediately. Missing files
    // return Ok(default), so this fires only on genuine failures.
    if !load_errs.is_empty() {
        ui.set_status_text(
            crate::i18n::dynamic::load_failed(localizer.loader(), &load_errs.join(" and ")).into(),
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
                // Resolve the bookmark to a present-in-library book path. A None
                // `last_opened`, or one whose path is no longer in `books` (the
                // book was purged), both yield None here and count as no bookmark.
                let path = {
                    let lib = library.borrow();
                    lib.last_opened()
                        .filter(|p| lib.books().iter().any(|book| book.path() == *p))
                        .map(std::path::Path::to_path_buf)
                };
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
    // In NORMAL mode a click only FOCUSES the cover (it never opens — opening is
    // Return-only). In SELECTION mode it focuses AND toggles the book's selection.
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
                // Look up the display title BEFORE removal — once removed the book
                // is gone from `books`, so the title must be captured first. A
                // signal for a path no longer present yields `None`; the removal
                // below then returns false and we bail out silently.
                let title = {
                    let lib = library.borrow();
                    lib.books()
                        .iter()
                        .find(|book| book.path() == path.as_path())
                        .map(|book| book.title().to_string())
                };
                let removed = library.borrow_mut().remove(&path);
                if !removed {
                    // Idempotency / race: the book was already removed by another
                    // path (its notice + rebuild already ran), so nothing to do.
                    return;
                }
                // `removed == true` guarantees `title` was Some (the book existed
                // when we read it just above), but fall back to the path's file
                // name defensively so the notice never shows an empty title.
                let title = title.unwrap_or_else(|| {
                    path.file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_default()
                });
                // Persist the removal. A save failure is surfaced (appended to the
                // notice), not just traced, mirroring the add/delete save-failure
                // handling; the in-memory removal stands either way.
                let save_error = match library.borrow().save() {
                    Ok(()) => None,
                    Err(e) => {
                        tracing::error!(error = %e, "failed to save library after empty-book auto-removal");
                        Some(format!("{e}"))
                    }
                };
                // Best-effort purge of the removed book's persistent cover,
                // mirroring the bulk-delete purge (mtime drift / missing file is
                // expected and only warned, never surfaced).
                match gashuu_core::ThumbnailCache::new() {
                    Ok(cache) => {
                        let purged = cache.purge_for(
                            &path,
                            cover_loader::mtime_secs(&path),
                            &[cover_loader::COVER_MAX_SIDE],
                        );
                        if purged == 0 {
                            tracing::warn!(
                                path = %path.display(),
                                "no persistent cover purged for auto-removed empty book (missing, mtime drift, or unwritable cache)"
                            );
                        }
                    }
                    Err(e) => {
                        tracing::warn!(error = %e, "cover cache unavailable; skipping cover purge on empty-book removal");
                    }
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
                let status = empty_book_removed_status(loader, &title, save_error.as_deref());
                ui.set_status_text(status.into());
            })
        });
    }

    // Thumbnail click: jump to the clicked page's spread, refresh, then restore
    // focus to the page area so keyboard navigation keeps working.
    {
        let ui_weak = ui.as_weak();
        let state = Rc::clone(&state);
        let viewport = Rc::clone(&viewport);
        let localizer = Rc::clone(&localizer);
        ui.on_thumbnail_clicked(move |page| {
            with_ui(&ui_weak, |ui| {
                if state.borrow_mut().jump_to(page as usize) {
                    refresh(&ui, &state.borrow(), &viewport, localizer.loader());
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
        let localizer = Rc::clone(&localizer);
        ui.on_page_jump_request(move |text: slint::SharedString| {
            with_ui(&ui_weak, |ui| {
                let total = state.borrow().page_count();
                let did_jump = if let Some(page_0based) = parse_page_jump(text.as_str(), total) {
                    let moved = state.borrow_mut().jump_to(page_0based);
                    if moved {
                        refresh(&ui, &state.borrow(), &viewport, localizer.loader());
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
        let localizer = Rc::clone(&localizer);
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
                refresh(&ui, &state.borrow(), &viewport, localizer.loader());
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
    // All three temporaries (state.borrow(), viewport.borrow(), settings.borrow_mut())
    // are argument expressions in the single reconcile_settings(...) call and drop
    // together at its `;`, before save()'s fresh settings.borrow() on the next line.
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
                // CURRENT book's per-book override).
                if ui.get_screen() == 0 {
                    reconcile_settings(
                        &state.borrow(),
                        &viewport.borrow(),
                        &mut settings.borrow_mut(),
                    );
                    if let Err(e) = settings.borrow().save() {
                        tracing::error!(error = %e, "failed to save settings from dialog");
                        ui.set_status_text(
                            crate::i18n::dynamic::could_not_save_settings(localizer.loader(), &e)
                                .into(),
                        );
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
                        ui.set_status_text(
                            crate::i18n::dynamic::could_not_save_settings(localizer.loader(), &e)
                                .into(),
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
                        tracing::error!(error = %e, "failed to save library on override reset");
                        ui.set_status_text(
                            crate::i18n::dynamic::could_not_save_settings(localizer.loader(), &e)
                                .into(),
                        );
                    }
                }
                // Apply the global defaults to the runtime + view.
                apply_global_view_to_runtime(&settings, &state, &viewport);
                refresh(&ui, &state.borrow(), &viewport, localizer.loader());
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
        let localizer = Rc::clone(&localizer);
        ui.on_set_reading_direction(move |i| {
            with_ui(&ui_weak, |ui| {
                let dir = index_to_reading_direction(i);
                // Mutates the runtime view mode only; while a book is open this change
                // is persisted to the current book's per-book override via
                // `write_back_view_override` at the next viewer leave point, not into
                // the global `Settings`.
                if state.borrow_mut().set_reading_direction(dir) {
                    refresh(&ui, &state.borrow(), &viewport, localizer.loader());
                }
            })
        });
    }
    {
        let ui_weak = ui.as_weak();
        let state = Rc::clone(&state);
        let viewport = Rc::clone(&viewport);
        let localizer = Rc::clone(&localizer);
        ui.on_set_spread_mode(move |i| {
            with_ui(&ui_weak, |ui| {
                let mode = index_to_spread_mode(i);
                // Mutates the runtime view mode only; while a book is open this change
                // is persisted to the current book's per-book override via
                // `write_back_view_override` at the next viewer leave point, not into
                // the global `Settings`.
                if state.borrow_mut().set_spread_mode(mode) {
                    refresh(&ui, &state.borrow(), &viewport, localizer.loader());
                }
            })
        });
    }
    {
        let ui_weak = ui.as_weak();
        let state = Rc::clone(&state);
        let viewport = Rc::clone(&viewport);
        let localizer = Rc::clone(&localizer);
        ui.on_set_cover_mode(move |i| {
            with_ui(&ui_weak, |ui| {
                let mode = index_to_cover_mode(i);
                // Mutates the runtime view mode only; while a book is open this change
                // is persisted to the current book's per-book override via
                // `write_back_view_override` at the next viewer leave point, not into
                // the global `Settings`.
                if state.borrow_mut().set_cover_mode(mode) {
                    refresh(&ui, &state.borrow(), &viewport, localizer.loader());
                }
            })
        });
    }
    {
        let ui_weak = ui.as_weak();
        let state = Rc::clone(&state);
        let viewport = Rc::clone(&viewport);
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
                    refresh(&ui, &state.borrow(), &viewport, localizer.loader());
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
    {
        let ui_weak = ui.as_weak();
        let state = Rc::clone(&state);
        let settings = Rc::clone(&settings);
        let viewport = Rc::clone(&viewport);
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
                refresh(&ui, &state.borrow(), &viewport, localizer.loader());
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
        let localizer = Rc::clone(&localizer);
        // The carousel-refresh collaborators are captured because the GoToLibrary
        // arm rebuilds the carousel on entry (continue-reading freshness +
        // focus snap) through `go_to_library` / `refresh_library_carousel`.
        let covers = Rc::clone(&covers);
        let search = Rc::clone(&search);
        let selection = Rc::clone(&selection);
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
                            refresh(&ui, &state.borrow(), &viewport, localizer.loader());
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
                            refresh(&ui, &state.borrow(), &viewport, localizer.loader());
                        }
                    }
                    KeyCommand::ToggleReadingDirection => {
                        if state.borrow_mut().toggle_reading_direction() {
                            refresh(&ui, &state.borrow(), &viewport, localizer.loader());
                        }
                    }
                    KeyCommand::ToggleCover => {
                        if state.borrow_mut().toggle_cover() {
                            refresh(&ui, &state.borrow(), &viewport, localizer.loader());
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
                        // `go_to_library` rebuilds the carousel on entry so the
                        // continue-reading ribbon reflects the `last_opened` just
                        // persisted above, and snaps focus to that book. The
                        // CarouselRefresh borrows are cheap `&Rc` references.
                        go_to_library(
                            &ui,
                            &nav,
                            &CarouselRefresh {
                                library: &library,
                                covers: &covers,
                                search: &search,
                                selection: &selection,
                                localizer: &localizer,
                            },
                        );
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
        let localizer = Rc::clone(&localizer);
        ui.on_resized(move |w, h| {
            with_ui(&ui_weak, |ui| {
                if state.borrow_mut().set_viewport_size(w, h) {
                    refresh(&ui, &state.borrow(), &viewport, localizer.loader());
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
    loader: &i18n_embed::fluent::FluentLanguageLoader,
) {
    let content = state.status_content();
    let status = crate::i18n::dynamic::format_status(loader, &content);
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
                Some(failed) => ui.set_status_text(
                    format!(
                        "{status}  {}",
                        crate::i18n::dynamic::page_unavailable(loader, failed + 1)
                    )
                    .into(),
                ),
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
            ui.set_status_text(crate::i18n::dynamic::decode_error(loader, &e).into());
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

/// Build the empty-book auto-removal status line: the localized
/// `empty_book_removed` notice, with the localized library-save-failure detail
/// appended via the shared `format!("{base} \u{2014} {detail}")` pattern when
/// `save_error` is `Some`. Shared by the two removal paths (the open-time
/// `finalize_open` arm and the cover-time `on_empty_book_detected` handler) so
/// the compose-and-append logic lives in one spot; the formatting stays in
/// `main.rs` (per the spec) while `empty_book_removed` / `failed_save_library`
/// remain the pure `dynamic.rs` notice seams.
fn empty_book_removed_status(
    loader: &i18n_embed::fluent::FluentLanguageLoader,
    title: &str,
    save_error: Option<&str>,
) -> String {
    let base = crate::i18n::dynamic::empty_book_removed(loader, title);
    match save_error {
        Some(e) => {
            let detail = crate::i18n::dynamic::failed_save_library(loader, &e);
            format!("{base} \u{2014} {detail}")
        }
        None => base,
    }
}

/// Finalize an `open_book.run(...)` outcome on the UI. On failure, set the
/// localized error status; on success, `refresh()` the view and append each
/// localized notice to the status line. The single place the four open sites
/// (Open Folder, Open Archive, carousel-open, bookmark-jump) share this UI
/// wiring, so the `OpenOutcome` match + notice-append loop lives in exactly one
/// spot.
fn finalize_open(
    ui: &ViewerWindow,
    state: &Rc<RefCell<ViewerState>>,
    viewport: &Rc<RefCell<ViewportState>>,
    deps: &CarouselRefresh,
    outcome: app::OpenOutcome,
) {
    let loader = deps.localizer.loader();
    match outcome {
        app::OpenOutcome::Error(e_str) => {
            ui.set_status_text(crate::i18n::dynamic::open_error_str(loader, &e_str).into());
        }
        app::OpenOutcome::Success(notices) => {
            refresh(ui, &state.borrow(), viewport, loader);
            for detail in crate::i18n::dynamic::format_notices(loader, &notices) {
                let base = ui.get_status_text().to_string();
                ui.set_status_text(format!("{base} \u{2014} {detail}").into());
            }
        }
        app::OpenOutcome::EmptyBookRemoved {
            title,
            removed,
            save_error,
        } => {
            // The source opened cleanly but has zero pages: the use case already
            // removed it from the library (if present) and re-saved. This arm does
            // NOT switch screens — the open-folder/archive sites only switch on a
            // user gesture, and the carousel-open/bookmark sites skip their
            // `go_to_viewer` for this variant (see the `enter_viewer` guard there),
            // so the user is left on a refreshed Library. Rebuild the carousel
            // through the shared chokepoint so the removed book disappears and the
            // cover-epoch bump drops any in-flight cover for it; the active search
            // filter is preserved by the chokepoint. Do NOT reset focus.
            refresh_library_carousel(ui, deps, false);
            if removed {
                // `removed == true` means THIS path performed the removal, so it
                // owns the notice. A concurrent path that already removed+notified
                // yields `removed == false` (idempotent) and stays silent below.
                let status = empty_book_removed_status(loader, &title, save_error.as_deref());
                ui.set_status_text(status.into());
            }
            // `removed == false`: another path already removed+notified this book
            // (race idempotency), so add no notice — but the carousel rebuild
            // above still ran, keeping this screen consistent.
        }
    }
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
///
/// On every entry transition this REBUILDS the carousel model through the shared
/// `refresh_library_carousel` chokepoint. The model was bound once at boot, so a
/// per-row flag derived from `last_opened` (the "continue reading" ribbon) would
/// otherwise still carry its build-time value: after reading book X and coming
/// back, `last_opened` has changed but the bound rows have not. Rebuilding here
/// re-derives those rows from the CURRENT library, restarting cover loading from
/// the (already-warm) cache and re-applying the path-keyed selection — both
/// handled inside the chokepoint, so neither flashes nor drops. We then snap the
/// carousel focus to the last-read book so Return resumes it. This is a ONE-SHOT
/// set at the entry moment, NOT a binding — after entry the user owns focus.
fn go_to_library(ui: &ViewerWindow, nav: &Rc<RefCell<NavState>>, deps: &CarouselRefresh) {
    nav.borrow_mut().to_library();
    ui.set_screen(screen_to_index(nav.borrow().screen()));
    // The Library's bottom strip renders `status-text` (its only consumer), so
    // the Viewer's page status ("12–13 / 200 [double · RTL]") written by
    // `refresh` would otherwise leak under the carousel, where it is
    // meaningless. Clear it on entry; add/save feedback is set AFTER this.
    ui.set_status_text("".into());
    // Rebuild so the model reflects the CURRENT `last_opened` (freshness), then
    // override the chokepoint's reset-to-0 with the continue-reading snap.
    // Two writes: refresh_library_carousel sets focused-index to 0 (reset_focus =
    // true), then the snap below sets the real target — the second always wins.
    // Keeping reset_focus = true documents that entry owns the focus, not the
    // residual viewer focus.
    refresh_library_carousel(ui, deps, true);
    snap_carousel_focus_to_last_opened(ui, deps.library, deps.search);
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

/// Outcome of an add batch: the canonical paths actually inserted (new books
/// only, in INPUT order) and the count of paths REJECTED because they could not
/// be opened as a book — either a source with zero image pages (the empty-book
/// rule) or an unreadable / unsupported source. Duplicates are NOT counted in
/// `skipped`: a path already in the library (or repeated within the batch) is
/// neither added nor rejected, mirroring `Library::add`'s `None`.
struct AddReport {
    added: Vec<std::path::PathBuf>,
    skipped: usize,
}

/// Add every path in `paths` to `lib`, rejecting sources that contain no images
/// before they ever enter the library.
///
/// Each path is FIRST probed with [`ArchiveLoader::probe_page_count`]:
/// - `Err(CoreError::EmptyBook { .. })` — the source opened but has zero image
///   pages: skip it and count it in `skipped` (the empty-book rule).
/// - `Err(other)` (I/O, `UnsupportedFormat`, …) — the source cannot be opened at
///   all: skip it, count it in `skipped`, and `warn!` with the error. A book that
///   cannot be opened is never added.
/// - `Ok(count)` — add via `Library::add` (which canonicalizes, dedups, and
///   re-sorts). On a genuine insert (`Some(canonical)`) the page count is recorded
///   immediately so a freshly added book shows "1 / N" without waiting for its
///   first open; a duplicate (`None`) is silently dropped (neither added nor
///   skipped).
fn add_paths(lib: &mut Library, paths: Vec<std::path::PathBuf>) -> AddReport {
    let mut added = Vec::new();
    let mut skipped = 0usize;
    for path in paths {
        match ArchiveLoader::probe_page_count(&path) {
            Err(CoreError::EmptyBook { .. }) => {
                skipped += 1;
                tracing::debug!(path = %path.display(), "skipping empty source (no image pages)");
            }
            Err(e) => {
                skipped += 1;
                tracing::warn!(error = %e, path = %path.display(), "skipping unreadable source");
            }
            Ok(count) => {
                if let Some(canonical) = lib.add(path).map(std::path::Path::to_path_buf) {
                    // Record the probed count on the freshly inserted book so it
                    // shows "1 / N" before its first open. `set_page_count`
                    // re-finds the book by its canonical path.
                    lib.set_page_count(&canonical, count);
                    added.push(canonical);
                }
                // `None` here means a duplicate (within the batch or already
                // present): neither added nor skipped, as before.
            }
        }
    }
    AddReport { added, skipped }
}

/// Resolve a VISIBLE carousel `index` to its underlying library book path,
/// through the search state's projection — the SAME hop `on_carousel_open` uses
/// (the carousel row is an index into `visible_indices`, which maps to a library
/// row). Returns `None` for an out-of-range index or a carousel/library desync.
/// Borrows `library` and `search` only for the duration of the call.
fn visible_index_to_path(
    library: &Rc<RefCell<Library>>,
    search: &Rc<RefCell<LibrarySearchState>>,
    index: i32,
) -> Option<std::path::PathBuf> {
    if index < 0 {
        return None;
    }
    let search = search.borrow();
    let library_index = search.visible_indices().get(index as usize).copied()?;
    let lib = library.borrow();
    lib.books()
        .get(library_index)
        .map(|book| book.path().to_path_buf())
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

/// Resolve the one-shot carousel `focused-index` to land on when ENTERING the
/// Library screen ("continue reading"): the VISIBLE row of `last_opened`, or `0`
/// as the fallback. The fallback covers every unresolvable case — no last-opened
/// book yet (`None`), the last-opened book filtered out of the current visible
/// set, an empty library, or a stale path no longer in `books`. Resolved THROUGH
/// the visible projection (`visible_indices` from the search state) so the index
/// is a carousel row, not a full-library index. Pure (library + projection in,
/// `i32` out) so it is headless-testable; the result is set ONCE at entry — after
/// that, user navigation owns `focused-index` (never a continuous binding).
fn entry_focus_index(lib: &Library, visible_indices: &[usize]) -> i32 {
    lib.last_opened()
        .and_then(|path| visible_focus_index_for_path(lib, visible_indices, path))
        .map(|index| index as i32)
        .unwrap_or(0)
}

/// Snap the carousel's `focused-index` to the last-read book ("continue reading")
/// when ENTERING the Library screen. A plain set at the entry moment — NOT a
/// binding — so user navigation owns focus afterwards. This OVERRIDES the
/// refresh's reset-to-0: a caller that just ran `refresh_library_carousel` with
/// `reset_focus = true` calls this immediately after, and the snap always wins.
/// Resolved through the CURRENT visible set (`entry_focus_index`); the borrow is
/// confined to the snap computation and drops before the UI set.
fn snap_carousel_focus_to_last_opened(
    ui: &ViewerWindow,
    library: &Rc<RefCell<Library>>,
    search: &Rc<RefCell<LibrarySearchState>>,
) {
    let focus = {
        let lib = library.borrow();
        entry_focus_index(&lib, search.borrow().visible_indices())
    };
    ui.set_carousel_focused_index(focus);
}

/// Clamp a carousel focused index into the valid range for a projection of
/// `visible_count` rows: `[0, visible_count - 1]`, or `0` when the projection is
/// empty. Pure so the destructive-delete refresh can pin the focused index to a
/// valid row BEFORE the Slint side reads it — an index past the shrunken
/// projection's end is the documented index-out-of-range crash risk. A negative
/// `old` (never produced by the live carousel, but defensive) floors to 0.
fn clamp_focused_index(old: i32, visible_count: usize) -> i32 {
    if visible_count == 0 {
        return 0;
    }
    let last = (visible_count - 1) as i32;
    old.clamp(0, last)
}

/// Push the selection-toolbar count text and select-all label into the UI.
///
/// Called from every point where the selection set or the visible projection
/// changes (toggle, select-all, exit, carousel rebuild, language switch, boot)
/// so the toolbar strings are always current without a full refresh.
///
/// Borrow discipline: `selection`, `search`, and `library` are distinct
/// `RefCell`s, so the three shared `Ref`s are taken together in one block scope
/// (both projection reads need the same trio) and drop at the block's `}` before
/// the UI setters run.
fn push_selection_strings(
    ui: &ViewerWindow,
    localizer: &i18n::Localizer,
    selection: &Rc<RefCell<LibrarySelectionState>>,
    search: &Rc<RefCell<LibrarySearchState>>,
    library: &Rc<RefCell<Library>>,
) {
    let loader = localizer.loader();
    // One shared-borrow group: `selection`, `search`, and `library` are distinct
    // `RefCell`s, so holding all three immutable `Ref`s at once is safe, and both
    // projection reads need the same trio. The group drops at the block's `}`.
    let (total, visible_selected, all_visible) = {
        let sel = selection.borrow();
        let srch = search.borrow();
        let lib = library.borrow();
        (
            sel.count(),
            sel.visible_selected_count(&srch, &lib),
            sel.all_visible_selected(&srch, &lib),
        )
    };
    ui.set_carousel_selection_count_text(
        crate::i18n::dynamic::selection_count_text(loader, total, visible_selected).into(),
    );
    ui.set_carousel_select_all_label(
        crate::i18n::dynamic::select_all_label(loader, all_visible).into(),
    );
    // The destructive toolbar twins: the pre-composed "Delete (N)…" label and the
    // `has-selection` gate (the DangerButton is disabled at N=0). Driven by the
    // TOTAL selection count, like the title, so they track every selection change.
    ui.set_carousel_delete_label(
        crate::i18n::dynamic::selection_delete_label(loader, total).into(),
    );
    ui.set_carousel_has_selection(total > 0);
}

/// The shared collaborators a Library carousel refresh threads together
/// (borrowed-collaborator bundle — same argument-count-cohesion intent as the
/// docs/patterns.md cohesion-wrapper flavor (`SpreadContext`), but holding `&Rc`
/// borrows rather than owned `Copy` values): the persisted `library`, the
/// `covers` stream controller, the `search` projection, the bulk-`selection`
/// state, and the `localizer` (for composing the selection-toolbar strings after
/// the projection changes). They ALWAYS travel together for a carousel rebuild,
/// so bundling them as borrows keeps `refresh_library_carousel` /
/// `add_books_and_refresh` under the argument-count limit and documents that
/// they are one collaboration unit, not independent params.
struct CarouselRefresh<'a> {
    library: &'a Rc<RefCell<Library>>,
    covers: &'a cover_loader::CoverController,
    search: &'a Rc<RefCell<LibrarySearchState>>,
    selection: &'a Rc<RefCell<LibrarySelectionState>>,
    localizer: &'a Rc<i18n::Localizer>,
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
fn refresh_library_carousel(ui: &ViewerWindow, deps: &CarouselRefresh, reset_focus: bool) {
    // Read everything the refresh needs under a single borrow, then drop it
    // before the UI mutations and `covers.start`.
    let (book_count, model, cover_reqs, indices) = {
        let lib = deps.library.borrow();
        let indices = deps.search.borrow().visible_indices().to_vec();
        (
            lib.books().len() as i32,
            build_carousel_model(&lib, &indices),
            cover_requests(&lib, &indices),
            indices,
        )
    };

    ui.set_library_book_count(book_count);
    // Idle bottom-strip label: the total library size, shown when no transient
    // notice occupies the strip (the count is the strip's idle state).
    ui.set_library_count_text(
        crate::i18n::dynamic::library_count_text(deps.localizer.loader(), book_count as usize)
            .into(),
    );
    bind_carousel_model(ui, model);
    if reset_focus {
        ui.set_carousel_focused_index(0);
    }
    // Re-apply the bulk selection over the freshly built (unselected) rows so a
    // selection survives a query change / add (selection is keyed by path, not
    // index). Reads `library` + `selection`; both `Ref`s drop before `covers.start`
    // (which takes a `borrow_mut` to persist prefetched counts).
    {
        let lib = deps.library.borrow();
        let selection = deps.selection.borrow();
        apply_selection_flags(ui, &lib, &indices, |path| selection.contains(path));
    }
    // Refresh the selection-toolbar strings: the visible projection just changed
    // (query change, add, boot), so visible_selected_count / all_visible_selected
    // may have moved. Both `Ref`s drop before `covers.start`.
    push_selection_strings(
        ui,
        deps.localizer,
        deps.selection,
        deps.search,
        deps.library,
    );
    // Dispatch covers nearest the focused row first: on a large library the
    // visible neighbourhood streams in immediately instead of queueing behind
    // hundreds of off-screen rows. Read the focus AFTER the reset above so a
    // reset-focus refresh starts from row 0. (The add path moves focus to the
    // new book only after this refresh; its cover is a fresh miss either way,
    // so ordering by the pre-add focus is fine there.)
    let focus_row = ui.get_carousel_focused_index().max(0) as usize;
    deps.covers.start(
        ui.as_weak(),
        deps.library,
        cover_loader::prioritize_by_focus(cover_reqs, focus_row),
    );
}

/// Which status notice to surface after `add_books_and_refresh` calls `add_paths`.
///
/// The four arms cover the full 2×2 of (added==0 vs added>0) × (skipped==0 vs
/// skipped>0).  The save-failure arm is handled separately in
/// `add_books_and_refresh` and is NOT part of this enum.
#[derive(Debug, PartialEq)]
enum AddNotice {
    /// All picked paths were already in the library (no new additions, no rejections).
    AlreadyInLibrary,
    /// Every path was rejected (no images or unreadable); nothing was added.
    NoneAddedAllSkipped { skipped: usize },
    /// Some books were added and some paths were rejected.
    AddedWithSkips { added: usize, skipped: usize },
    /// All picked paths were added successfully; none were rejected.
    Added { added: usize },
}

/// Pure decision function: maps the `(added, skipped)` counts from `add_paths`
/// to the appropriate [`AddNotice`] variant.  No I/O, no side-effects.
fn select_add_notice(added: usize, skipped: usize) -> AddNotice {
    match (added, skipped) {
        (0, 0) => AddNotice::AlreadyInLibrary,
        (0, s) => AddNotice::NoneAddedAllSkipped { skipped: s },
        (n, 0) => AddNotice::Added { added: n },
        (n, s) => AddNotice::AddedWithSkips {
            added: n,
            skipped: s,
        },
    }
}

/// Add `paths` to the library, persist, rebuild the filtered carousel, and
/// surface the outcome on the status line, restoring carousel focus in every
/// case.
///
/// Shared by the Add Books and Add Folder handlers; `op` distinguishes the two
/// only in the save-failure trace message. When nothing new is added there is
/// nothing to persist or rebuild, so it short-circuits after the status update.
///
/// Sources with no image pages (or that cannot be opened) are rejected by
/// `add_paths` before they enter the library; the status notice names how many
/// were skipped (added-some-skipped-some, or none-added-all-empty), falling back
/// to the already-in-library message only when the skip count is zero.
///
/// Newly added books are FORCED visible under the active filter (so an add never
/// silently hides the new book behind a non-matching query); the filter text
/// stays in place, and the forced override is cleared on the next user query
/// change (see `LibrarySearchState::set_query`).
fn add_books_and_refresh(
    ui: &ViewerWindow,
    deps: &CarouselRefresh,
    paths: Vec<std::path::PathBuf>,
    op: &'static str,
    loader: &i18n_embed::fluent::FluentLanguageLoader,
) {
    let AddReport {
        added: added_paths,
        skipped,
    } = add_paths(&mut deps.library.borrow_mut(), paths);
    if added_paths.is_empty() {
        // Nothing new entered the library: nothing to persist or rebuild. Route
        // through the pure decision fn so every branch is testable without Slint.
        let notice = match select_add_notice(0, skipped) {
            AddNotice::NoneAddedAllSkipped { skipped: s } => {
                crate::i18n::dynamic::no_books_added_empty(loader, s)
            }
            _ => crate::i18n::dynamic::already_in_library(loader),
        };
        ui.set_status_text(notice.into());
        ui.invoke_focus_carousel();
        return;
    }
    // Rebuild from the in-memory state even if the save fails, so the newly added
    // books are visible; the save error is then surfaced (not just traced). Keep
    // the just-added paths visible under the active filter, then refresh through
    // the shared chokepoint (which recomputes the filter, rebuilds + binds the
    // model, and restarts the cover stream). Focus is set explicitly below to the
    // new book's visible row, so do NOT reset focus to 0 here.
    let save_result = deps.library.borrow().save();
    // `search` and `library` are distinct RefCells, so the mut borrow of one and
    // the shared borrow of the other cannot conflict; the `library.borrow()`
    // drops at the `;` before refresh. `force_visible` recomputes internally, so
    // the visible set is consistent before `refresh_library_carousel` reads it.
    deps.search
        .borrow_mut()
        .force_visible(added_paths.clone(), &deps.library.borrow());
    refresh_library_carousel(ui, deps, false);
    match save_result {
        Err(e) => {
            tracing::error!(error = %e, "failed to save library after {op}");
            ui.set_status_text(
                crate::i18n::dynamic::added_books_save_failed(loader, added_paths.len(), &e).into(),
            );
        }
        Ok(()) => {
            // Some books were added; route through the pure decision fn so the
            // 4-way mapping is testable without Slint.
            let notice = match select_add_notice(added_paths.len(), skipped) {
                AddNotice::AddedWithSkips {
                    added: n,
                    skipped: s,
                } => crate::i18n::dynamic::added_books_skipped(loader, n, s),
                AddNotice::Added { added: n } => crate::i18n::dynamic::added_books(loader, n),
                // added_paths is non-empty here, so AlreadyInLibrary and
                // NoneAddedAllSkipped are unreachable; exhaustive for safety.
                _ => crate::i18n::dynamic::added_books(loader, added_paths.len()),
            };
            ui.set_status_text(notice.into());
        }
    }
    // Focus the first newly added book by its VISIBLE row (the carousel renders
    // the filtered slice), not its full-library index.
    if let Some(first_path) = added_paths.first() {
        let index = {
            let lib = deps.library.borrow();
            let search = deps.search.borrow();
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

    // ---- add_paths (empty-book rule) -------------------------------------
    //
    // Since the empty-book rule, `add_paths` PROBES each source before insert:
    // a source must contain at least one image page to be added. A folder is the
    // cheapest fixture — a zero-byte `*.png` counts as a page (listing is
    // extension-based), an empty folder probes to `EmptyBook`, and a nonexistent
    // path probes to an I/O error. These helpers build real temp dirs so probing
    // sees a genuine filesystem (the same reason the older tests already used
    // tempdirs: `Library::add` canonicalizes).

    /// Create a fresh temp directory under `parent/<name>` holding `pages`
    /// zero-byte `*.png` files (so it probes to a `pages`-page book). With
    /// `pages == 0` the directory is empty and probes to `EmptyBook`. Returns the
    /// directory path (its canonical form is what `Library::add` stores).
    fn make_book_dir(parent: &std::path::Path, name: &str, pages: usize) -> std::path::PathBuf {
        let dir = parent.join(name);
        std::fs::create_dir_all(&dir).expect("create book dir");
        for i in 0..pages {
            std::fs::write(dir.join(format!("page{i:03}.png")), []).expect("write page");
        }
        dir
    }

    /// Canonicalize a path the same way `Library::add` does, so test expectations
    /// match the stored/returned canonical paths.
    fn canon(path: &std::path::Path) -> std::path::PathBuf {
        path.canonicalize().expect("canonicalize existing path")
    }

    #[test]
    fn add_paths_empty_vec_returns_zero() {
        let mut lib = gashuu_core::Library::new();
        let report = add_paths(&mut lib, vec![]);
        assert!(report.added.is_empty());
        assert_eq!(report.skipped, 0);
        assert_eq!(lib.books().len(), 0);
    }

    #[test]
    fn add_paths_new_paths_counted() {
        let mut lib = gashuu_core::Library::new();
        let root = tempfile::tempdir().expect("tempdir");
        let vol1 = make_book_dir(root.path(), "vol1", 1);
        let vol2 = make_book_dir(root.path(), "vol2", 2);
        let report = add_paths(&mut lib, vec![vol1.clone(), vol2.clone()]);
        assert_eq!(report.added.len(), 2);
        assert_eq!(report.skipped, 0);
        assert_eq!(lib.books().len(), 2);
        // The returned vec holds the CANONICAL paths in INPUT order.
        assert_eq!(report.added, vec![canon(&vol1), canon(&vol2)]);
    }

    #[test]
    fn add_paths_dedup_within_batch() {
        let mut lib = gashuu_core::Library::new();
        let root = tempfile::tempdir().expect("tempdir");
        let vol1 = make_book_dir(root.path(), "vol1", 1);
        let report = add_paths(&mut lib, vec![vol1.clone(), vol1.clone()]);
        assert_eq!(
            report.added.len(),
            1,
            "duplicate within the batch must not be double-counted"
        );
        // A duplicate is neither added nor rejected, so it is NOT counted as skipped.
        assert_eq!(
            report.skipped, 0,
            "a duplicate is not an empty/unreadable skip"
        );
        assert_eq!(lib.books().len(), 1);
    }

    #[test]
    fn add_paths_dedup_against_existing() {
        let mut lib = gashuu_core::Library::new();
        let root = tempfile::tempdir().expect("tempdir");
        let vol1 = make_book_dir(root.path(), "vol1", 1);
        let vol2 = make_book_dir(root.path(), "vol2", 1);
        lib.add(vol1.clone());
        let report = add_paths(&mut lib, vec![vol1.clone(), vol2.clone()]);
        assert_eq!(
            report.added.len(),
            1,
            "a path already in the library must not be counted"
        );
        assert_eq!(
            report.skipped, 0,
            "an existing path is not an empty/unreadable skip"
        );
        assert_eq!(lib.books().len(), 2);
    }

    #[test]
    fn add_paths_returns_canonical_paths_and_skips_duplicates() {
        let mut lib = gashuu_core::Library::new();
        let root = tempfile::tempdir().expect("tempdir");
        let vol1 = make_book_dir(root.path(), "vol1", 1);
        // `vol1/.` and `vol1` canonicalize to the same path, so the second is a
        // duplicate and dropped.
        let with_dot = vol1.join(".");
        let expected = canon(&vol1);
        let report = add_paths(&mut lib, vec![with_dot.clone(), with_dot.clone()]);
        assert_eq!(report.added, vec![expected.clone()]);
        assert_eq!(report.skipped, 0);
        assert_eq!(lib.books().len(), 1);
        assert_eq!(lib.books()[0].path(), expected.as_path());
    }

    #[test]
    fn add_paths_all_existing_returns_zero() {
        let mut lib = gashuu_core::Library::new();
        let root = tempfile::tempdir().expect("tempdir");
        let vol1 = make_book_dir(root.path(), "vol1", 1);
        let vol2 = make_book_dir(root.path(), "vol2", 1);
        lib.add(vol1.clone());
        lib.add(vol2.clone());
        let before = lib.books().len();
        let report = add_paths(&mut lib, vec![vol1.clone(), vol2.clone()]);
        assert!(report.added.is_empty(), "all-duplicate batch must add 0");
        assert_eq!(report.skipped, 0, "duplicates are not skips");
        assert_eq!(lib.books().len(), before, "books count must not change");
    }

    #[test]
    fn add_paths_mixed_batch_counts_added_and_skipped() {
        // A valid book, an empty folder, and a duplicate of the valid book:
        // 1 added, 1 skipped (the empty), and the duplicate dropped silently.
        let mut lib = gashuu_core::Library::new();
        let root = tempfile::tempdir().expect("tempdir");
        let valid = make_book_dir(root.path(), "valid", 1);
        let empty = make_book_dir(root.path(), "empty", 0);
        let report = add_paths(&mut lib, vec![valid.clone(), empty.clone(), valid.clone()]);
        assert_eq!(
            report.added,
            vec![canon(&valid)],
            "only the valid book is added"
        );
        assert_eq!(report.skipped, 1, "the empty folder is the one skip");
        assert_eq!(lib.books().len(), 1);
        assert_eq!(lib.books()[0].path(), canon(&valid).as_path());
    }

    #[test]
    fn add_paths_all_empty_batch_adds_zero_skips_all() {
        // Every picked source is empty: nothing added, all counted as skipped.
        let mut lib = gashuu_core::Library::new();
        let root = tempfile::tempdir().expect("tempdir");
        let e1 = make_book_dir(root.path(), "e1", 0);
        let e2 = make_book_dir(root.path(), "e2", 0);
        let e3 = make_book_dir(root.path(), "e3", 0);
        let report = add_paths(&mut lib, vec![e1, e2, e3]);
        assert!(
            report.added.is_empty(),
            "no book added from an all-empty batch"
        );
        assert_eq!(report.skipped, 3, "all three empty sources are skipped");
        assert_eq!(lib.books().len(), 0);
    }

    #[test]
    fn add_paths_unreadable_path_is_skipped() {
        // A nonexistent path cannot be opened (I/O error), so it is rejected as a
        // skip — never added (an "unreadable" source is NOT classified as empty,
        // but is still kept out of the library).
        let mut lib = gashuu_core::Library::new();
        let report = add_paths(
            &mut lib,
            vec![std::path::PathBuf::from(
                "/nonexistent_gashuu_add_paths_unreadable",
            )],
        );
        assert!(report.added.is_empty(), "an unreadable path is never added");
        assert_eq!(
            report.skipped, 1,
            "the unreadable path is counted as skipped"
        );
        assert_eq!(lib.books().len(), 0);
    }

    #[test]
    fn add_paths_sets_page_count_immediately() {
        // A freshly added book carries its probed page count so the carousel can
        // show "1 / N" before the book is ever opened.
        let mut lib = gashuu_core::Library::new();
        let root = tempfile::tempdir().expect("tempdir");
        let three = make_book_dir(root.path(), "three", 3);
        let report = add_paths(&mut lib, vec![three.clone()]);
        assert_eq!(report.added.len(), 1);
        assert_eq!(report.skipped, 0);
        let book = lib
            .books()
            .iter()
            .find(|b| b.path() == canon(&three))
            .expect("added book present");
        assert_eq!(
            book.page_count_opt(),
            Some(3),
            "the probed page count is recorded on add"
        );
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

    // ---- entry_focus_index (continue reading) ----------------------------

    /// Build a 3-book library (alpha/beta/gamma, natural order) and mark `index`
    /// as last-opened via `register_opened`. Returns the library; the natural
    /// order is alpha(0), beta(1), gamma(2) since the paths sort lexically.
    fn library_with_last_opened(index: usize) -> Library {
        let mut lib = Library::new();
        for leaf in ["alpha", "beta", "gamma"] {
            assert!(lib
                .add(std::path::PathBuf::from(format!("/manga/{leaf}.cbz")))
                .is_some());
        }
        let path = lib.books()[index].path().to_path_buf();
        lib.register_opened(&path, None);
        lib
    }

    #[test]
    fn entry_focus_index_resolves_last_opened_with_no_filter() {
        // beta is last-opened and all three rows are visible (natural-order
        // identity projection), so the snap lands on beta's row (1).
        let lib = library_with_last_opened(1);
        assert_eq!(entry_focus_index(&lib, &[0, 1, 2]), 1);
    }

    #[test]
    fn entry_focus_index_uses_filtered_position() {
        // gamma is last-opened and the visible set is the filtered slice
        // [beta, gamma]; gamma's VISIBLE row is 1, not its library index 2.
        let lib = library_with_last_opened(2);
        assert_eq!(entry_focus_index(&lib, &[1, 2]), 1);
    }

    #[test]
    fn entry_focus_index_falls_back_when_last_opened_filtered_out() {
        // alpha is last-opened but the filter hides it (visible set is gamma
        // only), so the snap falls back to row 0.
        let lib = library_with_last_opened(0);
        assert_eq!(entry_focus_index(&lib, &[2]), 0);
    }

    #[test]
    fn entry_focus_index_falls_back_when_no_last_opened() {
        // A fresh library has never opened a book, so the fallback is row 0.
        let mut lib = Library::new();
        assert!(lib
            .add(std::path::PathBuf::from("/manga/alpha.cbz"))
            .is_some());
        assert_eq!(lib.last_opened(), None, "no book opened yet");
        assert_eq!(entry_focus_index(&lib, &[0]), 0);
    }

    #[test]
    fn entry_focus_index_falls_back_for_empty_library() {
        // No books and no visible rows: the fallback is row 0 (never panics on
        // the empty slice).
        let lib = Library::new();
        assert_eq!(entry_focus_index(&lib, &[]), 0);
    }

    // ---- clamp_focused_index (bulk-delete focus safety) -------------------

    #[test]
    fn clamp_focused_index_pins_into_shrunken_projection() {
        // The crash guard: after a bulk delete shrinks the projection, a focused
        // index past the new last row must clamp DOWN to the last valid row.
        assert_eq!(clamp_focused_index(7, 3), 2, "past-end clamps to last row");
        // An empty projection (everything deleted) floors to 0.
        assert_eq!(clamp_focused_index(7, 0), 0, "empty projection floors to 0");
        assert_eq!(clamp_focused_index(0, 0), 0, "0 on empty stays 0");
        // An in-range index is preserved unchanged.
        assert_eq!(clamp_focused_index(1, 4), 1, "in-range index unchanged");
        assert_eq!(clamp_focused_index(0, 1), 0, "single row keeps focus at 0");
        // A negative index (defensive; not produced by the live carousel) floors.
        assert_eq!(clamp_focused_index(-1, 4), 0, "negative floors to 0");
    }

    #[test]
    fn add_paths_returns_input_order_while_books_are_natural_order() {
        // Focus follows the FIRST input path, not natural order: `add_paths`
        // returns the inserted paths in INPUT order, whereas `lib.books()` keeps
        // them in NATURAL (sorted) order. Both share one parent dir so their leaf
        // names (vol1, vol10) drive the natural sort.
        let mut lib = gashuu_core::Library::new();
        let root = tempfile::tempdir().expect("tempdir");
        let vol10 = canon(&make_book_dir(root.path(), "vol10", 1));
        let vol1 = canon(&make_book_dir(root.path(), "vol1", 1));
        let report = add_paths(&mut lib, vec![vol10.clone(), vol1.clone()]);

        // Returned vec is in INPUT order (vol10 first, vol1 second).
        assert_eq!(report.added[0], vol10);
        assert_eq!(report.added[1], vol1);

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

    // ---- select_add_notice (reject-empty-books status routing) --------------

    #[test]
    fn select_add_notice_already_in_library_when_both_zero() {
        assert_eq!(select_add_notice(0, 0), AddNotice::AlreadyInLibrary);
    }

    #[test]
    fn select_add_notice_none_added_all_skipped_when_added_zero_skipped_nonzero() {
        assert_eq!(
            select_add_notice(0, 3),
            AddNotice::NoneAddedAllSkipped { skipped: 3 }
        );
    }

    #[test]
    fn select_add_notice_added_with_skips_when_both_nonzero() {
        assert_eq!(
            select_add_notice(2, 1),
            AddNotice::AddedWithSkips {
                added: 2,
                skipped: 1
            }
        );
    }

    #[test]
    fn select_add_notice_added_when_added_nonzero_skipped_zero() {
        assert_eq!(select_add_notice(5, 0), AddNotice::Added { added: 5 });
    }
}
