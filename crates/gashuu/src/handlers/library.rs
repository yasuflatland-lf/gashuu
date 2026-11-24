use crate::backend::{BackendError, BookBackend, BookImporter, OpenedBook};
use crate::book_path::{locate, request_id};
use crate::dialog_session::{DialogScope, DialogSession};
use crate::picker::{forget_uploads, BookPicker, PickedBook, UploadHandle};
use crate::{
    add_controller,
    carousel::{apply_selection_flags, set_carousel_selected},
    cover_loader, i18n, open_book, remove_books,
};
use crate::{
    apply_add_report, current_book_name, finalize_empty_book_rejected, finalize_open,
    finalize_remove, go_to_viewer, push_selection_toolbar_state, refresh_library_carousel,
    save_library, visible_index_to_id, with_ui, CarouselRefresh, LibraryStoreHandle, ViewerWindow,
};
use crate::{
    library_model::{LibrarySearchState, LibrarySelectionState},
    navigation::NavState,
    open_controller::OpenController,
    page_loader::PageController,
    selection_projection,
    thumbnail_strip::ThumbnailController,
};
use crate::{viewer_state::ViewerState, viewport::ViewportState};
use gashuu_core::BookId;
use gashuu_core::{Library, Settings, ThumbStore};
use slint::ComponentHandle;
use std::cell::{Cell, RefCell};
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::{Arc, Mutex};

type PickedSlot = Arc<Mutex<Vec<PickedBook>>>;

fn lock_picks(slot: &PickedSlot) -> std::sync::MutexGuard<'_, Vec<PickedBook>> {
    slot.lock().expect("picked slot mutex poisoned")
}

fn deliver_picks(
    weak: &slint::Weak<ViewerWindow>,
    slot: &PickedSlot,
    folder: bool,
    picks: Vec<PickedBook>,
) {
    // A rejected web selection can have a notice even though it has no handles.
    #[cfg(target_arch = "wasm32")]
    let has_notice = !folder && crate::picker::web::has_notice();
    #[cfg(not(target_arch = "wasm32"))]
    let has_notice = false;
    if picks.is_empty() && !has_notice {
        return;
    }
    let ui = weak.upgrade();
    let stale = if ui.is_some() {
        std::mem::replace(&mut *lock_picks(slot), picks)
    } else {
        picks
    };
    forget_uploads(&partition_picks(stale).1);
    if let Some(ui) = ui {
        ui.invoke_add_picked(folder);
    }
}

fn partition_picks(picks: Vec<PickedBook>) -> (Vec<PathBuf>, Vec<UploadHandle>) {
    let mut paths = Vec::new();
    let mut uploads = Vec::new();
    for pick in picks {
        match pick {
            PickedBook::Path(path) => paths.push(path),
            PickedBook::Upload(file) => uploads.push(file),
        }
    }
    debug_assert!(paths.is_empty() || uploads.is_empty());
    (paths, uploads)
}

/// Registers the add-books/add-folder callbacks onto `ui`.
/// Panel constraint (#151): explicit handle list IS the dependency list — no AppState bundle.
#[allow(clippy::too_many_arguments)]
pub(crate) fn wire_open_handlers(
    ui: &ViewerWindow,
    picker: &Rc<dyn BookPicker>,
    importer: Option<Rc<dyn BookImporter>>,
    settings: &Rc<RefCell<Settings>>,
    library: &Rc<RefCell<Library>>,
    covers: &Rc<cover_loader::CoverController>,
    adder: &Rc<add_controller::AddController>,
    search: &Rc<RefCell<LibrarySearchState>>,
    selection: &Rc<RefCell<LibrarySelectionState>>,
    localizer: &Rc<i18n::Localizer>,
    library_store: &LibraryStoreHandle,
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
    let library_store = Rc::clone(library_store);
    // One carousel-refresh bundle for this fn's callbacks; each callback captures a clone.
    let carousel = CarouselRefresh {
        library,
        library_store,
        covers,
        search,
        selection,
        localizer: Rc::clone(&localizer),
    };

    let picked_slot = PickedSlot::default();
    let pending_folder = Rc::new(Cell::new(None));
    {
        let picker = Rc::clone(picker);
        let weak = ui.as_weak();
        let slot = picked_slot.clone();
        ui.on_add_books(move || {
            let weak = weak.clone();
            let slot = slot.clone();
            picker.pick_books(Box::new(move |picks| {
                deliver_picks(&weak, &slot, false, picks)
            }));
        });
    }
    {
        let picker = Rc::clone(picker);
        let weak = ui.as_weak();
        let slot = picked_slot.clone();
        ui.on_add_folder(move || {
            let weak = weak.clone();
            let slot = slot.clone();
            picker.pick_folder(Box::new(move |pick| {
                deliver_picks(&weak, &slot, true, pick.into_iter().collect());
            }));
        });
    }
    {
        let weak = ui.as_weak();
        let adder = adder.clone();
        let picked_slot = picked_slot.clone();
        let pending_folder = pending_folder.clone();
        #[cfg(target_arch = "wasm32")]
        let localizer = localizer.clone();
        ui.on_add_picked(move |folder| {
            pending_folder.take();
            let picks = std::mem::take(&mut *lock_picks(&picked_slot));
            let (paths, uploads) = partition_picks(picks);
            let op = if folder { "add-folder" } else { "add-books" };
            let policy = settings.borrow().archive_policy();
            if !paths.is_empty() {
                if !uploads.is_empty() {
                    // A mixed selection is impossible for platform pickers. In release,
                    // finish the path batch before starting the uploads.
                    *lock_picks(&picked_slot) =
                        uploads.into_iter().map(PickedBook::Upload).collect();
                    pending_folder.set(Some(folder));
                }
                adder.start(weak.clone(), paths, policy, op);
                return;
            }
            #[cfg(target_arch = "wasm32")]
            let notice = crate::picker::web::take_notice(&localizer);
            #[cfg(not(target_arch = "wasm32"))]
            let notice: Option<String> = None;

            if !uploads.is_empty() {
                if let Some(importer) = importer.as_deref() {
                    adder.start_import(weak.clone(), uploads, policy, op, notice, importer);
                    return;
                }
                forget_uploads(&uploads);
                if let Some(ui) = weak.upgrade() {
                    #[cfg(target_arch = "wasm32")]
                    if let Err(error) = crate::backend::web::shared() {
                        let message = i18n::dynamic::backend_error_text(localizer.loader(), &error);
                        ui.set_add_toast_text(append_notice(message, notice).into());
                    }
                    ui.invoke_focus_carousel();
                }
            } else if let (Some(ui), Some(notice)) = (weak.upgrade(), notice) {
                ui.set_status_text("".into());
                ui.set_add_toast_text(notice.into());
                ui.invoke_focus_carousel();
            }
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
    // `None` = superseded), then mutate + save the library together on the UI thread
    // before running the rest of the add tail.
    {
        let ui_weak = ui.as_weak();
        let adder = Rc::clone(&adder);
        let localizer = Rc::clone(&localizer);
        let carousel = carousel.clone();
        ui.on_add_finalize(move |epoch| {
            with_ui(&ui_weak, |ui| {
                let Some((outcomes, op)) = adder.take_outcomes(epoch.max(0) as usize) else {
                    return;
                };
                #[cfg(target_arch = "wasm32")]
                let import_notice = import_failure_notice(&outcomes, &localizer);
                apply_add_report(&ui, &carousel, outcomes, op, localizer.loader());
                let notice = adder.take_picker_notice();
                #[cfg(target_arch = "wasm32")]
                let notice = [notice, import_notice]
                    .into_iter()
                    .flatten()
                    .reduce(|a, b| format!("{a}\n{b}"));
                if let Some(notice) = notice {
                    ui.set_add_toast_text(
                        append_notice(ui.get_add_toast_text().to_string(), Some(notice)).into(),
                    );
                }
                // This generation's add is complete — hide the progress hairline.
                // (Superseded finalizes returned early above, so a newer in-flight
                // add's bar is left running for its own finalize to retire.)
                ui.set_add_active(false);
                if let Some(folder) = pending_folder.take() {
                    ui.invoke_add_picked(folder);
                }
            })
        });
    }
}

#[cfg(target_arch = "wasm32")]
fn import_failure_notice(
    outcomes: &[add_controller::ProbeOutcome],
    localizer: &i18n::Localizer,
) -> Option<String> {
    use add_controller::{ProbeKind, ProbeSubject};
    use gashuu_worker::import::{STORAGE_FULL, UNREADABLE_UPLOAD};

    let mut notices = Vec::new();
    let loader = localizer.loader();
    let mut unreadable = 0;
    for outcome in outcomes {
        if let (ProbeSubject::Upload(_), ProbeKind::Unreadable { error }) =
            (&outcome.subject, &outcome.kind)
        {
            let notice = match error {
                BackendError::Other { detail } if detail == UNREADABLE_UPLOAD => {
                    unreadable += 1;
                    continue;
                }
                BackendError::Other { detail } if detail == STORAGE_FULL => {
                    i18n::dynamic::web_storage_full(loader)
                }
                BackendError::StorageUnavailable
                | BackendError::WorkerUnavailable
                | BackendError::BookLocked
                | BackendError::Missing { .. } => i18n::dynamic::backend_error_text(loader, error),
                _ => continue,
            };
            if !notices.contains(&notice) {
                notices.push(notice);
            }
        }
    }
    if unreadable > 0 {
        notices.push(i18n::dynamic::web_unreadable_files(loader, unreadable));
    }
    (!notices.is_empty()).then(|| notices.join("\n"))
}

fn append_notice(report: String, notice: Option<String>) -> String {
    match notice {
        Some(notice) if report.is_empty() => notice,
        Some(notice) => format!("{report}\n{notice}"),
        None => report,
    }
}

fn open_and_enter(
    ui: &ViewerWindow,
    open_ctrl: &OpenController,
    settings: &RefCell<Settings>,
    state: &RefCell<ViewerState>,
    localizer: &i18n::Localizer,
    path: std::path::PathBuf,
) {
    if let Err(error) = start_open(
        ui.as_weak(),
        open_ctrl,
        &path,
        settings.borrow().archive_policy(),
        state.borrow().cache_config(),
    ) {
        ui.set_status_text(i18n::dynamic::open_error(localizer.loader(), &error).into());
    }
}

fn start_open(
    weak: slint::Weak<ViewerWindow>,
    ctrl: &OpenController,
    path: &std::path::Path,
    policy: gashuu_core::ArchivePolicy,
    cache: gashuu_core::CacheConfig,
) -> Result<(), BackendError> {
    ctrl.start(weak, request_id(path)?, policy, cache);
    Ok(())
}

/// UI-7: a visible dialog with an active session must close before an open is applied.
fn dialog_must_close_before_open(show_settings: bool, scope: Option<DialogScope>) -> bool {
    show_settings && scope.is_some()
}

/// Close an active settings dialog through its registered close handler. Returns whether it closed.
fn close_settings_dialog_if_active(
    ui: &ViewerWindow,
    dialog_session: &RefCell<DialogSession>,
) -> bool {
    let show_settings = ui.get_show_settings();
    let scope = dialog_session.borrow().scope();
    let should_close = dialog_must_close_before_open(show_settings, scope);
    if should_close {
        ui.invoke_close_settings();
    }
    should_close
}

/// End any in-progress settings-dialog session, then apply the backend open result.
///
/// ORDER IS LOAD-BEARING, and this pairing is why the two statements live in one
/// function: `apply_opened` opens with the `OpenDifferentBook` leave-point transaction,
/// which writes the CURRENT runtime onto the OUTGOING book. A settings dialog
/// opened on the Library screen leaves that runtime seeded with the GLOBAL
/// defaults, so an open that lands while the dialog is up used to overwrite the
/// previous book's own view modes with the globals (issue #535, path (b)).
/// Ending also commits a Library-scope global edit before restoring that book
/// runtime, so an async open cannot discard the edit when it clears the session
/// (issue #556). It mutates `Settings` in memory; existing save points are unchanged.
///
/// Extracted from the `on_open_finalize` closure so the ordering is reachable
/// without a live window; the borrow stays confined to the one statement.
fn end_session_and_apply_opened(
    dialog_session: &RefCell<DialogSession>,
    state: &RefCell<ViewerState>,
    viewport: &RefCell<ViewportState>,
    settings: &RefCell<Settings>,
    open_book: &open_book::OpenBookUseCase,
    request: &BookId,
    result: Result<OpenedBook, BackendError>,
) -> open_book::OpenOutcome {
    dialog_session.borrow_mut().end(state, viewport, settings);
    open_book.apply_opened(request, result)
}

/// Registers the library-search and carousel navigation/open callbacks onto `ui`.
/// Panel constraint (#151): explicit handle list IS the dependency list — no AppState bundle.
#[allow(clippy::too_many_arguments)]
pub(crate) fn wire_carousel_handlers(
    ui: &ViewerWindow,
    state: &Rc<RefCell<ViewerState>>,
    viewport: &Rc<RefCell<ViewportState>>,
    dialog_session: &Rc<RefCell<DialogSession>>,
    library: &Rc<RefCell<Library>>,
    nav: &Rc<RefCell<NavState>>,
    settings: &Rc<RefCell<Settings>>,
    open_book: &Rc<open_book::OpenBookUseCase>,
    open_ctrl: &Rc<OpenController>,
    covers: &Rc<cover_loader::CoverController>,
    pages: &Rc<PageController>,
    thumbs: &Rc<ThumbnailController>,
    search: &Rc<RefCell<LibrarySearchState>>,
    selection: &Rc<RefCell<LibrarySelectionState>>,
    localizer: &Rc<i18n::Localizer>,
    library_store: &LibraryStoreHandle,
) {
    // Rebind the `&Rc<_>` params to owned `Rc` locals so each closure's `Rc::clone(&handle)`
    // prelude stays byte-identical to its pre-extraction form in `main`.
    let state = Rc::clone(state);
    let viewport = Rc::clone(viewport);
    let dialog_session = Rc::clone(dialog_session);
    let library = Rc::clone(library);
    let nav = Rc::clone(nav);
    let settings = Rc::clone(settings);
    let open_book = Rc::clone(open_book);
    let open_ctrl = Rc::clone(open_ctrl);
    let covers = Rc::clone(covers);
    let pages = Rc::clone(pages);
    let thumbs = Rc::clone(thumbs);
    let search = Rc::clone(search);
    let selection = Rc::clone(selection);
    let localizer = Rc::clone(localizer);
    let library_store = Rc::clone(library_store);
    // One carousel-refresh bundle for this fn's callbacks; each callback captures a clone.
    let carousel = CarouselRefresh {
        library: Rc::clone(&library),
        library_store,
        covers,
        search: Rc::clone(&search),
        selection,
        localizer: Rc::clone(&localizer),
    };

    // Single-open finalize: drain only the current epoch, then run the complete
    // mutation/persistence/UI tail on the event-loop thread.
    {
        let ui_weak = ui.as_weak();
        let open_ctrl = Rc::clone(&open_ctrl);
        let nav = Rc::clone(&nav);
        let open_book = Rc::clone(&open_book);
        let state = Rc::clone(&state);
        let viewport = Rc::clone(&viewport);
        let dialog_session = Rc::clone(&dialog_session);
        let settings = Rc::clone(&settings);
        let pages = Rc::clone(&pages);
        let thumbs = Rc::clone(&thumbs);
        let carousel = carousel.clone();
        ui.on_open_finalize(move |epoch| {
            with_ui(&ui_weak, |ui| {
                let Some(pending) = open_ctrl.take_outcome(epoch.max(0) as usize) else {
                    return;
                };
                close_settings_dialog_if_active(&ui, &dialog_session);
                let (request, result) = pending.into_result();
                let outcome = end_session_and_apply_opened(
                    &dialog_session,
                    &state,
                    &viewport,
                    &settings,
                    &open_book,
                    &request,
                    result,
                );
                // Enter the Viewer ONLY on a clean open: an empty source was already
                // removed, and a failed open must keep the user on the Library screen.
                let enter_viewer = matches!(outcome, open_book::OpenOutcome::Success { .. });
                finalize_open(&ui, &state, &viewport, &pages, &thumbs, &carousel, outcome);
                ui.set_current_book_name(current_book_name(&state).into());
                if enter_viewer {
                    go_to_viewer(&ui, &nav);
                }
            })
        });
    }

    // Library search: the debounced NavBar query (the ONLY query-update path). Replace the
    // filter, rebuild the filtered carousel + cover stream, reset focus to row 0.
    {
        let ui_weak = ui.as_weak();
        let library = Rc::clone(&library);
        let search = Rc::clone(&search);
        let carousel = carousel.clone();
        ui.on_library_search_changed(move |query| {
            with_ui(&ui_weak, |ui| {
                // `search` and `library` are distinct RefCells, so a mut borrow of one and a
                // shared borrow of the other can't conflict; the shared borrow drops before refresh.
                search
                    .borrow_mut()
                    .set_query(query.to_string(), &library.borrow());
                // The selection (keyed by path) is ORTHOGONAL to the query: the rebuild
                // re-applies it over the new visible rows, so a query change never drops a book.
                refresh_library_carousel(&ui, &carousel, true);
            })
        });
    }

    // Carousel open: Return on the focused book (or a double-click on the CENTER cover,
    // fired as the same `open(int)`); resumes its last-read page and enters the Viewer.
    {
        let ui_weak = ui.as_weak();
        let library = Rc::clone(&library);
        let search = Rc::clone(&search);
        let settings = Rc::clone(&settings);
        let open_ctrl = Rc::clone(&open_ctrl);
        let localizer = Rc::clone(&localizer);
        let state = Rc::clone(&state);
        ui.on_carousel_open(move |index| {
            with_ui(&ui_weak, |ui| {
                // Resolve the VISIBLE carousel index to a book ID via the search
                // projection — the same hop others use, single-homed in `visible_index_to_id`.
                let Some(id) = visible_index_to_id(&library, &search, index) else {
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
                open_and_enter(&ui, &open_ctrl, &settings, &state, &localizer, locate(&id));
            })
        });
    }

    // Bookmark capsule: jump to the continue-reading book (`last_opened`). If still on
    // the shelf, open via the same path as Return; else (None/stale) show the no-bookmark notice.
    {
        let ui_weak = ui.as_weak();
        let library = Rc::clone(&library);
        let localizer = Rc::clone(&localizer);
        let settings = Rc::clone(&settings);
        let open_ctrl = Rc::clone(&open_ctrl);
        let state = Rc::clone(&state);
        ui.on_carousel_continue_reading(move || {
            with_ui(&ui_weak, |ui| {
                // `Library::bookmark()` returns `last_opened` only when still on the shelf;
                // a purged book yields None, counting as no bookmark (notice below).
                let id = library.borrow().bookmark().cloned();
                let Some(id) = id else {
                    // No registered bookmark — answer the click with a notice so
                    // the always-enabled capsule still gives feedback.
                    ui.set_status_text(
                        crate::i18n::dynamic::bookmark_none(localizer.loader()).into(),
                    );
                    return;
                };
                open_and_enter(&ui, &open_ctrl, &settings, &state, &localizer, locate(&id));
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
                if state.borrow().open_book().is_none() {
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
    viewport: &Rc<RefCell<ViewportState>>,
    dialog_session: &Rc<RefCell<DialogSession>>,
    settings: &Rc<RefCell<Settings>>,
    library: &Rc<RefCell<Library>>,
    covers: &Rc<cover_loader::CoverController>,
    search: &Rc<RefCell<LibrarySearchState>>,
    selection: &Rc<RefCell<LibrarySelectionState>>,
    localizer: &Rc<i18n::Localizer>,
    library_store: &LibraryStoreHandle,
    backend: Rc<dyn BookBackend>,
    importer: Option<Rc<dyn crate::backend::BookImporter>>,
    thumbs: Arc<dyn ThumbStore>,
) {
    // Rebind the `&Rc<_>` params to owned `Rc` locals so each closure's `Rc::clone(&handle)`
    // prelude stays byte-identical to its pre-extraction form in `main`.
    let state = Rc::clone(state);
    let dialog_session = Rc::clone(dialog_session);
    let library = Rc::clone(library);
    let covers = Rc::clone(covers);
    let search = Rc::clone(search);
    let selection = Rc::clone(selection);
    let localizer = Rc::clone(localizer);
    let library_store = Rc::clone(library_store);
    // One carousel-refresh bundle for this fn's callbacks; each callback captures a clone.
    let carousel = CarouselRefresh {
        library: Rc::clone(&library),
        library_store: Rc::clone(&library_store),
        covers,
        search: Rc::clone(&search),
        selection: Rc::clone(&selection),
        localizer: Rc::clone(&localizer),
    };

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
                let Some(id) = visible_index_to_id(&library, &search, index) else {
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
                selection.borrow_mut().toggle(id.clone());
                let selected = selection.borrow().contains(&id);
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
                let Some(id) = visible_index_to_id(&library, &search, index) else {
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
                selection.borrow_mut().toggle(id.clone());
                let selected = selection.borrow().contains(&id);
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
                    remove_books::confirm_delete_content(
                        localizer.loader(),
                        &selection.borrow(),
                        &search.borrow(),
                        &library.borrow(),
                        st.open_book(),
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
        let carousel = carousel.clone();
        let remove_books = remove_books::RemoveBooksUseCase::new(
            Rc::clone(&state),
            Rc::clone(viewport),
            Rc::clone(&dialog_session),
            Rc::clone(settings),
            Rc::clone(&library),
            Rc::clone(&search),
            Rc::clone(&selection),
            Rc::clone(&library_store),
            backend,
            importer.clone(),
            Arc::clone(&thumbs),
        );
        ui.on_confirm_delete_accepted(move || {
            with_ui(&ui_weak, |ui| {
                let outcome = remove_books.run();
                // Dismiss the modal in every outcome (its stale content props are
                // rebuilt on the next open).
                ui.set_show_confirm_delete(false);
                finalize_remove(&ui, &carousel, outcome);
            })
        });
    }

    // A cover worker found a book with zero image pages: auto-remove it, persist, purge
    // its cover, rebuild the carousel, and notice. Idempotent (second signal = no-op).
    {
        let ui_weak = ui.as_weak();
        let library = Rc::clone(&library);
        let library_store = Rc::clone(&library_store);
        let carousel = carousel.clone();
        ui.on_empty_book_detected(move |id_str| {
            with_ui(&ui_weak, |ui| {
                let id = BookId::new(id_str.as_str());
                // The shared transaction (single home in open_book): title capture
                // BEFORE removal -> Library::remove -> save -> best-effort cover purge.
                let removal = open_book::remove_empty_book(
                    &library,
                    &id,
                    None,
                    thumbs.as_ref(),
                    importer.as_deref(),
                    |library| save_library(&library_store, library),
                );
                finalize_empty_book_rejected(&ui, &carousel, &removal);
            })
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::open_book::opened;
    use gashuu_core::{
        CoverMode, FitMode, ReadingDirection, ResolvedView, SpreadMode, ViewOverride,
    };

    fn make_book_dir(parent: &std::path::Path, name: &str, pages: usize) -> std::path::PathBuf {
        let source = parent.join(name);
        std::fs::create_dir(&source).expect("create fixture book");
        for page in 0..pages {
            std::fs::write(source.join(format!("page{page:03}.png")), [])
                .expect("write fixture page");
        }
        source
    }

    #[test]
    fn dialog_must_close_before_open_only_with_an_active_session() {
        for (show_settings, scope, expected) in [
            (true, Some(DialogScope::Library), true),
            (true, Some(DialogScope::Viewer), true),
            (true, None, false),
            (false, Some(DialogScope::Library), false),
            (false, None, false),
        ] {
            assert_eq!(
                dialog_must_close_before_open(show_settings, scope),
                expected,
                "show_settings={show_settings}, scope={scope:?}"
            );
        }
    }

    #[test]
    fn close_settings_dialog_if_active_invokes_the_close_handler_once() {
        let ui = crate::cover_loader::test_window();
        ui.set_show_settings(true);
        let settings = RefCell::new(Settings::default());
        let state = RefCell::new(ViewerState::new());
        let viewport = RefCell::new(ViewportState::from_settings(&settings.borrow()));
        let dialog_session = Rc::new(RefCell::new(DialogSession::new()));
        dialog_session
            .borrow_mut()
            .open(DialogScope::Library, &state, &viewport, &settings);
        let calls = Rc::new(Cell::new(0));
        ui.on_close_settings({
            let calls = Rc::clone(&calls);
            let dialog_session = Rc::clone(&dialog_session);
            move || {
                calls.set(calls.get() + 1);
                dialog_session
                    .borrow_mut()
                    .end(&state, &viewport, &settings);
            }
        });

        assert!(close_settings_dialog_if_active(&ui, &dialog_session));
        assert_eq!(calls.get(), 1);
        // The stub leaves the dialog visible to exercise the inactive-session guard.
        assert!(!close_settings_dialog_if_active(&ui, &dialog_session));
        assert_eq!(calls.get(), 1);
    }

    #[test]
    fn closing_a_library_dialog_before_an_open_commits_globals_only() {
        use crate::view_sync::ViewModeRoute;

        let root = tempfile::tempdir().expect("tempdir");
        let first = make_book_dir(root.path(), "first", 3);
        let second = make_book_dir(root.path(), "second", 3);
        let (first_id, first_title) = crate::book_path::identify(&first, true).unwrap();
        let (second_id, second_title) = crate::book_path::identify(&second, true).unwrap();
        let settings = Rc::new(RefCell::new(Settings {
            reading_direction: ReadingDirection::Rtl,
            spread_mode: SpreadMode::Double,
            cover_mode: CoverMode::Paired,
            fit_mode: FitMode::Actual,
            ..Settings::default()
        }));
        let book_view = ResolvedView {
            reading_direction: ReadingDirection::Ltr,
            spread_mode: SpreadMode::Single,
            cover_mode: CoverMode::Standalone,
            fit_mode: FitMode::Whole,
        };
        let edited_global = ResolvedView {
            reading_direction: ReadingDirection::Rtl,
            spread_mode: SpreadMode::Auto,
            cover_mode: CoverMode::Paired,
            fit_mode: FitMode::Width,
        };
        let incoming_override = ViewOverride {
            reading_direction: Some(ReadingDirection::Ltr),
            spread_mode: Some(SpreadMode::Double),
            cover_mode: Some(CoverMode::Paired),
            fit_mode: Some(FitMode::Actual),
        };
        let incoming_view = incoming_override.resolve(edited_global);
        let state = Rc::new(RefCell::new(ViewerState::new()));
        state.borrow_mut().open_path(&first).expect("open first");
        let viewport = Rc::new(RefCell::new(ViewportState::from_settings(
            &settings.borrow(),
        )));
        state
            .borrow_mut()
            .apply_resolved_view(book_view, &mut viewport.borrow_mut());
        let mut library_value = Library::new();
        assert!(library_value.add(first_id.clone(), first_title).is_some());
        assert!(library_value.add(second_id.clone(), second_title).is_some());
        assert!(library_value.set_overrides(&second_id, incoming_override));
        let library = Rc::new(RefCell::new(library_value));
        let dialog_session = Rc::new(RefCell::new(DialogSession::new()));
        let leave_point = Rc::new(crate::LeavePointService::new(
            Rc::clone(&state),
            Rc::clone(&viewport),
            Rc::clone(&dialog_session),
            Rc::clone(&settings),
            Rc::clone(&library),
            Rc::new(None),
        ));
        let open_book = open_book::OpenBookUseCase::new(
            Rc::clone(&state),
            Rc::clone(&settings),
            Rc::clone(&viewport),
            Rc::clone(&dialog_session),
            Rc::clone(&library),
            Rc::clone(&leave_point),
            Box::new(|_| Ok(())),
            Box::new(|_| Ok(())),
            crate::backend::fake::FakeBackend::new(),
            None,
            Arc::new(gashuu_core::NullThumbStore),
        );

        dialog_session
            .borrow_mut()
            .open(DialogScope::Library, &state, &viewport, &settings);
        state
            .borrow_mut()
            .apply_resolved_view(edited_global, &mut viewport.borrow_mut());
        leave_point
            .persist_with(ViewModeRoute::DialogClosedOnLibrary, |_| Ok(()))
            .expect("persist Library dialog close");
        dialog_session
            .borrow_mut()
            .end(&state, &viewport, &settings);

        let (request, probe) = opened(&second);
        let outcome = end_session_and_apply_opened(
            &dialog_session,
            &state,
            &viewport,
            &settings,
            &open_book,
            &request,
            probe,
        );

        assert!(matches!(outcome, open_book::OpenOutcome::Success { .. }));
        assert_eq!(settings.borrow().view_defaults(), edited_global);
        assert_eq!(
            library.borrow().overrides_for(&first_id),
            ViewOverride::differences_from(book_view, edited_global)
        );
        assert_eq!(
            crate::view_sync::current_runtime_view(&state, &viewport),
            incoming_view
        );
        assert_eq!(
            library.borrow().overrides_for(&second_id),
            incoming_override
        );
        assert_eq!(dialog_session.borrow().scope(), None);
    }

    #[cfg(unix)]
    #[test]
    fn open_and_enter_non_utf8_path_does_not_dispatch() {
        use std::os::unix::ffi::OsStrExt;
        let backend = crate::backend::fake::FakeBackend::new();
        let ctrl = OpenController::new(backend.clone());
        let path = std::path::Path::new(std::ffi::OsStr::from_bytes(b"book\xff"));
        assert!(start_open(
            Default::default(),
            &ctrl,
            path,
            Default::default(),
            Default::default()
        )
        .is_err());
        assert!(backend.calls().is_empty());
    }

    /// Issue #535, path (b), pinned at the PRODUCTION call site: an open that lands
    /// while a settings dialog opened on the Library screen is still up must persist
    /// the OUTGOING book's own view modes, not the global-seeded scratchpad the
    /// dialog installed. Deleting the `end()` call from `end_session_and_apply_opened`
    /// stages the globals onto the outgoing book instead.
    #[test]
    fn open_finalize_with_a_library_dialog_open_persists_the_outgoing_books_own_modes() {
        let root = tempfile::tempdir().expect("tempdir");
        let first = make_book_dir(root.path(), "first", 3);
        let second = make_book_dir(root.path(), "second", 3);

        // Globals (G) and the outgoing book's own modes (B) differ in all four axes.
        let settings = Rc::new(RefCell::new(Settings {
            reading_direction: ReadingDirection::Rtl,
            spread_mode: SpreadMode::Double,
            cover_mode: CoverMode::Paired,
            fit_mode: FitMode::Actual,
            ..Settings::default()
        }));
        let state = Rc::new(RefCell::new(ViewerState::new()));
        state.borrow_mut().open_path(&first).expect("open first");
        let canonical = state
            .borrow()
            .open_book()
            .expect("open file after successful open")
            .clone();
        let viewport = Rc::new(RefCell::new(ViewportState::from_settings(
            &settings.borrow(),
        )));
        state.borrow_mut().apply_resolved_view(
            ResolvedView {
                reading_direction: ReadingDirection::Ltr,
                spread_mode: SpreadMode::Single,
                cover_mode: CoverMode::Standalone,
                fit_mode: FitMode::Whole,
            },
            &mut viewport.borrow_mut(),
        );
        let mut library_value = Library::new();
        assert!(library_value
            .add(canonical.clone(), "book".into())
            .is_some());
        let library = Rc::new(RefCell::new(library_value));
        let dialog_session = Rc::new(RefCell::new(DialogSession::new()));
        let leave_point = Rc::new(crate::LeavePointService::new(
            Rc::clone(&state),
            Rc::clone(&viewport),
            Rc::clone(&dialog_session),
            Rc::clone(&settings),
            Rc::clone(&library),
            Rc::new(None),
        ));
        // Hermetic: both persistence effects are injected no-ops, so nothing reaches
        // the process data directory.
        let open_book = open_book::OpenBookUseCase::new(
            Rc::clone(&state),
            Rc::clone(&settings),
            Rc::clone(&viewport),
            Rc::clone(&dialog_session),
            Rc::clone(&library),
            leave_point,
            Box::new(|_| Ok(())),
            Box::new(|_| Ok(())),
            crate::backend::fake::FakeBackend::new(),
            None,
            Arc::new(gashuu_core::NullThumbStore),
        );
        // The Library-screen dialog seeds the runtime with G, then the in-flight
        // open of the SECOND book lands.
        dialog_session
            .borrow_mut()
            .open(DialogScope::Library, &state, &viewport, &settings);
        let (request, probe) = opened(&second);
        let outcome = end_session_and_apply_opened(
            &dialog_session,
            &state,
            &viewport,
            &settings,
            &open_book,
            &request,
            probe,
        );

        assert!(matches!(outcome, open_book::OpenOutcome::Success { .. }));
        assert_eq!(
            library.borrow().overrides_for(&canonical),
            ViewOverride {
                reading_direction: Some(ReadingDirection::Ltr),
                spread_mode: Some(SpreadMode::Single),
                cover_mode: Some(CoverMode::Standalone),
                fit_mode: Some(FitMode::Whole),
            },
            "the outgoing book keeps its own modes, not the dialog's global scratchpad"
        );
        assert!(
            dialog_session.borrow().scope().is_none(),
            "the open-finalize path ends the session"
        );
    }

    #[test]
    fn open_finalize_during_a_library_dialog_keeps_the_global_edit() {
        let root = tempfile::tempdir().expect("tempdir");
        let first = make_book_dir(root.path(), "first", 3);
        let second = make_book_dir(root.path(), "second", 3);
        let (first_canonical, first_title) = crate::book_path::identify(&first, true).unwrap();
        let (second_canonical, second_title) = crate::book_path::identify(&second, true).unwrap();

        // G, G', and B differ wherever the enum cardinality permits; B and G'
        // differ in all four axes so reconcile/restore order cannot pass by chance.
        let settings = Rc::new(RefCell::new(Settings {
            reading_direction: ReadingDirection::Rtl,
            spread_mode: SpreadMode::Double,
            cover_mode: CoverMode::Paired,
            fit_mode: FitMode::Actual,
            ..Settings::default()
        }));
        let book_view = ResolvedView {
            reading_direction: ReadingDirection::Ltr,
            spread_mode: SpreadMode::Single,
            cover_mode: CoverMode::Standalone,
            fit_mode: FitMode::Whole,
        };
        let edited_global = ResolvedView {
            reading_direction: ReadingDirection::Rtl,
            spread_mode: SpreadMode::Auto,
            cover_mode: CoverMode::Paired,
            fit_mode: FitMode::Width,
        };
        let incoming_view = ResolvedView {
            reading_direction: ReadingDirection::Ltr,
            spread_mode: SpreadMode::Double,
            cover_mode: CoverMode::Paired,
            fit_mode: FitMode::Actual,
        };
        let state = Rc::new(RefCell::new(ViewerState::new()));
        state.borrow_mut().open_path(&first).expect("open first");
        let viewport = Rc::new(RefCell::new(ViewportState::from_settings(
            &settings.borrow(),
        )));
        state
            .borrow_mut()
            .apply_resolved_view(book_view, &mut viewport.borrow_mut());
        let mut library_value = Library::new();
        assert!(library_value
            .add(first_canonical.clone(), first_title)
            .is_some());
        assert!(library_value
            .add(second_canonical.clone(), second_title)
            .is_some());
        assert!(library_value.set_overrides(
            &second_canonical,
            ViewOverride {
                reading_direction: Some(incoming_view.reading_direction),
                spread_mode: Some(incoming_view.spread_mode),
                cover_mode: Some(incoming_view.cover_mode),
                fit_mode: Some(incoming_view.fit_mode),
            }
        ));
        let library = Rc::new(RefCell::new(library_value));
        let dialog_session = Rc::new(RefCell::new(DialogSession::new()));
        let leave_point = Rc::new(crate::LeavePointService::new(
            Rc::clone(&state),
            Rc::clone(&viewport),
            Rc::clone(&dialog_session),
            Rc::clone(&settings),
            Rc::clone(&library),
            Rc::new(None),
        ));
        let open_book = open_book::OpenBookUseCase::new(
            Rc::clone(&state),
            Rc::clone(&settings),
            Rc::clone(&viewport),
            Rc::clone(&dialog_session),
            Rc::clone(&library),
            leave_point,
            Box::new(|_| Ok(())),
            Box::new(|_| Ok(())),
            crate::backend::fake::FakeBackend::new(),
            None,
            Arc::new(gashuu_core::NullThumbStore),
        );

        dialog_session
            .borrow_mut()
            .open(DialogScope::Library, &state, &viewport, &settings);
        state
            .borrow_mut()
            .apply_resolved_view(edited_global, &mut viewport.borrow_mut());
        let (request, probe) = opened(&second);
        let outcome = end_session_and_apply_opened(
            &dialog_session,
            &state,
            &viewport,
            &settings,
            &open_book,
            &request,
            probe,
        );

        assert!(matches!(outcome, open_book::OpenOutcome::Success { .. }));
        assert_eq!(
            library.borrow().overrides_for(&first_canonical),
            ViewOverride {
                reading_direction: Some(book_view.reading_direction),
                spread_mode: Some(book_view.spread_mode),
                cover_mode: Some(book_view.cover_mode),
                fit_mode: Some(book_view.fit_mode),
            },
            "the outgoing book keeps its own view"
        );
        assert_eq!(
            crate::view_sync::current_runtime_view(&state, &viewport),
            incoming_view,
            "the incoming book resolves its own override"
        );
        let settings = settings.borrow();
        assert_eq!(settings.reading_direction, edited_global.reading_direction);
        assert_eq!(settings.spread_mode, edited_global.spread_mode);
        assert_eq!(settings.cover_mode, edited_global.cover_mode);
        assert_eq!(settings.fit_mode, edited_global.fit_mode);
    }
}
