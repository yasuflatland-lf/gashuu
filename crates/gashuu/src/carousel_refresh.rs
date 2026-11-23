//! Library carousel refresh + projection: rebuild/bind the filtered carousel
//! model, (re)start cover loading for the visible rows, and map between visible
//! carousel rows and library book IDs.
//!
//! Extracted from `main.rs` (mirrors the `view_sync.rs` split): these items form
//! one cohesive unit — the [`CarouselRefresh`] collaborator bundle and every
//! function that consumes it — driven almost entirely from `handlers::library`
//! and `handlers::settings`. `go_to_library` / `go_to_viewer` stay in `main.rs`
//! and route their carousel work through here. UI-thread only.

use crate::add_books::{apply_outcomes_and_save, select_add_notice, AddNotice, AddReport};
use crate::add_controller;
use crate::carousel::{
    apply_selection_flags, bind_carousel_model, build_carousel_model, cover_requests,
};
use crate::library_model::{LibrarySearchState, LibrarySelectionState};
use crate::open_book::EmptyBookOutcome;
use crate::remove_books::RemoveOutcome;
use crate::{cover_loader, i18n, save_library, LibraryStoreHandle, ViewerWindow};
use gashuu_core::BookId;
use gashuu_core::Library;
use slint::ComponentHandle;
use std::cell::RefCell;
use std::rc::Rc;

/// Resolve a VISIBLE carousel `index` to its underlying book ID,
/// through the search state's projection — the SAME hop `on_carousel_open` uses
/// (the carousel row is an index into `visible_indices`, which maps to a library
/// row). Returns `None` for an out-of-range index or a carousel/library desync.
/// Borrows `library` and `search` only for the duration of the call.
pub(crate) fn visible_index_to_id(
    library: &RefCell<Library>,
    search: &RefCell<LibrarySearchState>,
    index: i32,
) -> Option<BookId> {
    if index < 0 {
        return None;
    }
    let search = search.borrow();
    let library_index = search.visible_indices().get(index as usize).copied()?;
    let lib = library.borrow();
    lib.books().get(library_index).map(|book| book.id().clone())
}

/// Return the 0-based VISIBLE (filtered) carousel row index of `id`, or `None`
/// if `id` is not among the currently visible rows. `visible_indices` is the
/// search state's projection (library rows in natural order that pass the filter
/// or are forced visible); the returned position is the index INTO that slice,
/// i.e. the carousel row to focus.
pub(crate) fn visible_focus_index_for_id(
    lib: &Library,
    visible_indices: &[usize],
    id: &BookId,
) -> Option<usize> {
    visible_indices.iter().position(|&library_index| {
        lib.books()
            .get(library_index)
            .is_some_and(|book| book.id() == id)
    })
}

/// Resolve the one-shot carousel `focused-index` to land on when ENTERING the
/// Library screen ("continue reading"): the VISIBLE row of `last_opened`, or `0`
/// as the fallback. The fallback covers every unresolvable case — no last-opened
/// book yet (`None`), the last-opened book filtered out of the current visible
/// set, an empty library, or a stale ID no longer in `books`. Resolved THROUGH
/// the visible projection (`visible_indices` from the search state) so the index
/// is a carousel row, not a full-library index. Pure (library + projection in,
/// `i32` out) so it is headless-testable; the result is set ONCE at entry — after
/// that, user navigation owns `focused-index` (never a continuous binding).
fn entry_focus_index(lib: &Library, visible_indices: &[usize]) -> i32 {
    lib.last_opened()
        .and_then(|id| visible_focus_index_for_id(lib, visible_indices, id))
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
pub(crate) fn snap_carousel_focus_to_last_opened(
    ui: &ViewerWindow,
    library: &RefCell<Library>,
    search: &RefCell<LibrarySearchState>,
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
pub(crate) fn clamp_focused_index(old: i32, visible_count: usize) -> i32 {
    if visible_count == 0 {
        return 0;
    }
    let last = (visible_count - 1) as i32;
    old.clamp(0, last)
}

/// Push the full selection-toolbar state into the UI: the count text, the
/// select-all label, the pre-composed delete label, AND the `has-selection`
/// boolean gate (not just the strings, hence the name).
///
/// Called from every point where the selection set or the visible projection
/// changes (toggle, select-all, exit, carousel rebuild, language switch, boot)
/// so the toolbar state is always current without a full refresh.
///
/// Borrow discipline: `selection`, `search`, and `library` are distinct
/// `RefCell`s, so the three shared `Ref`s are taken together in one block scope
/// (both projection reads need the same trio) and drop at the block's `}` before
/// the UI setters run.
pub(crate) fn push_selection_toolbar_state(
    ui: &ViewerWindow,
    localizer: &i18n::Localizer,
    selection: &RefCell<LibrarySelectionState>,
    search: &RefCell<LibrarySearchState>,
    library: &RefCell<Library>,
) {
    let loader = localizer.loader();
    // One shared-borrow group: `selection`/`search`/`library` are distinct RefCells, so
    // holding all three immutable Refs at once is safe. The group drops at the block's `}`.
    let (total, visible_selected, all_visible) = {
        let sel = selection.borrow();
        let srch = search.borrow();
        let lib = library.borrow();
        (
            sel.count(),
            crate::selection_projection::visible_selected_count(&sel, &srch, &lib),
            crate::selection_projection::all_visible_selected(&sel, &srch, &lib),
        )
    };
    ui.set_carousel_selection_count_text(
        crate::i18n::dynamic::selection_count_text(loader, total, visible_selected).into(),
    );
    ui.set_carousel_select_all_label(
        crate::i18n::dynamic::select_all_label(loader, all_visible).into(),
    );
    // The destructive toolbar twins: the pre-composed "Delete (N)…" label and the
    // `has-selection` gate (DangerButton disabled at N=0), driven by the TOTAL selection count.
    ui.set_carousel_delete_label(
        crate::i18n::dynamic::selection_delete_label(loader, total).into(),
    );
    ui.set_carousel_has_selection(total > 0);
}

/// The shared collaborators a Library carousel refresh threads together: the
/// persisted `library` and its `library_store`, the `covers` stream controller,
/// the `search` projection, the bulk-`selection` state, and the `localizer` (for
/// composing the selection-toolbar strings after the projection changes). They
/// ALWAYS travel together for a carousel rebuild, so one bundle keeps
/// `refresh_library_carousel` / `apply_add_report` under the argument-count
/// limit and documents that they are one collaboration unit.
///
/// Owns its `Rc` handles (as `OpenBookUseCase` does) rather than borrowing them:
/// a Slint callback closure must be `'static`, so a borrowed bundle could not be
/// captured and every callback had to clone all six handles and re-assemble the
/// bundle inside its body. Each `wire_*` fn builds ONE bundle from its explicit
/// handle list and each callback captures one `carousel.clone()` (six refcount
/// bumps, no allocation).
#[derive(Clone)]
pub(crate) struct CarouselRefresh {
    pub(crate) library: Rc<RefCell<Library>>,
    pub(crate) library_store: LibraryStoreHandle,
    pub(crate) covers: Rc<cover_loader::CoverController>,
    pub(crate) search: Rc<RefCell<LibrarySearchState>>,
    pub(crate) selection: Rc<RefCell<LibrarySelectionState>>,
    pub(crate) localizer: Rc<i18n::Localizer>,
}

/// Project the CURRENT (already-recomputed) search state into the carousel:
/// rebuild + bind the filtered carousel model, optionally reset carousel focus
/// to row 0, and (re)start cover loading for the visible rows. The SINGLE place
/// the carousel + cover stream are refreshed from the shared search state, shared
/// by the initial boot build, the debounced query callback, and book additions
/// and removals.
///
/// This function only READS `visible_indices()`; it does NOT recompute. Every
/// caller mutates the search state through a recomputing entry point first —
/// `set_query` (startup seed and query changes), `force_visible` (add), or
/// `recompute` (empty-book removal and bulk delete).
///
/// Borrow discipline: all reads share ONE `library.borrow()` scope that drops
/// before the UI bind and `covers.start` (which takes a `borrow_mut` to persist
/// any prefetched page counts) — never hold a `borrow()` across `start`.
pub(crate) fn refresh_library_carousel(
    ui: &ViewerWindow,
    deps: &CarouselRefresh,
    reset_focus: bool,
) {
    // Read everything the refresh needs under a single borrow, then drop it
    // before the UI mutations and `covers.start`.
    let (book_count, model, cover_reqs, indices) = {
        let lib = deps.library.borrow();
        let indices = deps.search.borrow().visible_indices().to_vec();
        (
            lib.books().len() as i32,
            build_carousel_model(&lib, &indices, &crate::book_availability::book_path_exists),
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
    // Re-apply bulk selection over the freshly built (unselected) rows so a selection
    // survives a query change / add (keyed by ID, not index). Refs drop before covers.start.
    {
        let lib = deps.library.borrow();
        let selection = deps.selection.borrow();
        apply_selection_flags(ui, &lib, &indices, |id| selection.contains(id));
    }
    // Refresh the selection-toolbar state: the visible projection just changed (query,
    // add, boot), so the visible/all-selected counts may have moved.
    push_selection_toolbar_state(
        ui,
        &deps.localizer,
        &deps.selection,
        &deps.search,
        &deps.library,
    );
    // Dispatch covers nearest the focused row first so a large library streams the visible
    // neighbourhood immediately. Read focus AFTER the reset above so reset-focus starts at row 0.
    let focus_row = ui.get_carousel_focused_index().max(0) as usize;
    deps.covers.start(
        ui.as_weak(),
        &deps.library,
        &deps.library_store,
        cover_loader::prioritize_by_focus(cover_reqs, focus_row),
    );
}

fn prune_selection_to_shelf(
    selection: &RefCell<LibrarySelectionState>,
    library: &RefCell<Library>,
) -> usize {
    let removed_ids: Vec<BookId> = {
        let library = library.borrow();
        let selection = selection.borrow();
        selection
            .selected()
            .filter(|id| library.book(id).is_none())
            .cloned()
            .collect()
    };
    let removed_count = removed_ids.len();
    for id in removed_ids {
        selection.borrow_mut().remove(&id);
    }
    removed_count
}

/// The shared UI tail of an empty-book auto-removal, called by both the open-time
/// `finalize_open` arm and the cover-time `on_empty_book_detected` handler so the
/// two cannot drift. After a successful removal, prunes the selection before
/// the refresh so bulk selection is always a subset of books on the shelf.
/// Recomputes the projection because row indices shift after removal, then
/// rebuilds the carousel through the chokepoint (removed book
/// disappears; the cover-epoch bump drops any sibling cover still streaming for
/// it; active filter preserved; no focus reset), then — only when THIS path
/// performed the removal (`removed == true`) — surfaces the auto-removal notice
/// (with the library-save-failure detail appended). The `removed`-idempotency
/// guard is single-sourced here: a `removed == false` (race, or a never-added
/// empty source) rebuilds but stays silent.
pub(crate) fn finalize_empty_book_rejected(
    ui: &ViewerWindow,
    deps: &CarouselRefresh,
    removal: &EmptyBookOutcome,
) {
    if removal.removed {
        prune_selection_to_shelf(&deps.selection, &deps.library);
    }
    deps.search.borrow_mut().recompute(&deps.library.borrow());
    refresh_library_carousel(ui, deps, false);
    if removal.removed {
        let status = crate::empty_book_removed_status(
            deps.localizer.loader(),
            &removal.title,
            removal.save_error.as_deref(),
        );
        ui.set_status_text(status.into());
    }
}

/// Finalize a `RemoveBooksUseCase::run()` outcome on the UI — the mirror of
/// `finalize_open`. The use case stays lean (returns data); this owns the Slint
/// tail per outcome arm, so the destructive path's UI finalize lives in one
/// place and cannot drift from the open path's.
pub(crate) fn finalize_remove(ui: &ViewerWindow, deps: &CarouselRefresh, outcome: RemoveOutcome) {
    let loader = deps.localizer.loader();
    match outcome {
        RemoveOutcome::NoSelection => {
            // Defensive: the request handler guards against an empty selection.
            ui.invoke_focus_carousel();
        }
        RemoveOutcome::SaveFailed { error } => {
            // Shelf rolled back and selection PRESERVED by `run` (rows unchanged);
            // stay in selection mode so the user can retry.
            ui.set_status_text(crate::i18n::dynamic::delete_save_failed(loader, &error).into());
            ui.invoke_focus_carousel();
        }
        RemoveOutcome::Removed {
            n,
            closed_open_book,
        } => {
            // `run` closed the viewer's open book (headless) when it was deleted;
            // blank the centered title-bar name here (the Slint side of that close).
            if closed_open_book {
                ui.set_current_book_name("".into());
            }
            // `run` already recomputed the projection and cleared the selection. Rebuild from
            // the fresh visible set (no focus reset — the focused index is clamped below).
            refresh_library_carousel(ui, deps, false);
            #[cfg(target_arch = "wasm32")]
            ui.invoke_storage_inventory_changed();
            // Clamp the focused index into the NEW visible row count BEFORE Slint can
            // read a stale out-of-range value (index-out-of-range is the crash risk).
            let visible_count = deps.search.borrow().visible_indices().len();
            let clamped = clamp_focused_index(ui.get_carousel_focused_index(), visible_count);
            ui.set_carousel_focused_index(clamped);
            // Exit selection mode: drop the toolbar and re-apply all-false flags
            // (selection itself was already cleared by `run`).
            ui.set_carousel_selection_mode(false);
            {
                let lib = deps.library.borrow();
                let indices = deps.search.borrow().visible_indices().to_vec();
                apply_selection_flags(ui, &lib, &indices, |_| false);
            }
            push_selection_toolbar_state(
                ui,
                &deps.localizer,
                &deps.selection,
                &deps.search,
                &deps.library,
            );
            // Status LAST (status-last ordering): the deleted-books notice. `n`
            // already excludes stale IDs.
            ui.set_status_text(crate::i18n::dynamic::deleted_books(loader, n).into());
            ui.invoke_focus_carousel();
        }
    }
}

/// Route a neutral / success add notice to the quiet idle strip. Clears any
/// error toast so the two channels never show duplicate text.
fn set_status_strip(ui: &ViewerWindow, text: String) {
    ui.set_add_toast_text("".into());
    ui.set_status_text(text.into());
}

/// Route a skip / failure add notice to the attention-grabbing toast. Clears the
/// strip's transient text so it falls back to the idle library count (no dup).
fn set_add_toast(ui: &ViewerWindow, text: String) {
    ui.set_status_text("".into());
    ui.set_add_toast_text(text.into());
}

/// Apply already-probed add `outcomes` to the library, persist, rebuild the
/// filtered carousel, and surface the outcome on the status line, restoring
/// carousel focus in every case. The UI-thread tail of the bulk add (issue 206),
/// run from the `add-finalize` handler once the off-thread probe completes.
///
/// Shared by the Add Books and Add Folder paths; `op` distinguishes the two only
/// in the save-failure trace message. When nothing new was added there is nothing
/// to persist or rebuild, so it short-circuits after the status update.
///
/// Sources with no image pages (or that cannot be opened) were rejected by the
/// probe + `apply_outcomes` before they entered the library; the status notice
/// names how many were skipped (added-some-skipped-some, or none-added-all-empty),
/// falling back to the already-in-library message only when the skip count is zero.
///
/// Newly added books are FORCED visible under the active filter (so an add never
/// silently hides the new book behind a non-matching query); the filter text
/// stays in place, and the forced override is cleared on the next user query
/// change (see `LibrarySearchState::set_query`).
pub(crate) fn apply_add_report(
    ui: &ViewerWindow,
    deps: &CarouselRefresh,
    outcomes: Vec<add_controller::ProbeOutcome>,
    op: &'static str,
    loader: &i18n_embed::fluent::FluentLanguageLoader,
) {
    let (report, save_result) =
        apply_outcomes_and_save(&mut deps.library.borrow_mut(), outcomes, |library| {
            save_library(&deps.library_store, library)
        });
    let AddReport {
        added: added_ids,
        skipped,
    } = report;
    if added_ids.is_empty() {
        // Nothing new entered the library: nothing to persist or rebuild. Route
        // through the pure decision fn so every branch is testable without Slint.
        match select_add_notice(0, skipped) {
            AddNotice::NoneAddedAllSkipped { skipped: s } => {
                // Every picked path was rejected: surface it on the toast so the
                // failure is noticed, not lost in the quiet strip.
                set_add_toast(ui, crate::i18n::dynamic::no_books_added_empty(loader, s));
            }
            _ => {
                // "Already in library" is neutral info → quiet strip.
                set_status_strip(ui, crate::i18n::dynamic::already_in_library(loader));
            }
        }
        ui.invoke_focus_carousel();
        return;
    }
    // Rebuild from in-memory state even if the save fails, so the added books are visible
    // (the save error is surfaced below). Focus is set explicitly later, so don't reset to 0.
    // `search` and `library` are distinct RefCells, so this mut+shared borrow can't conflict.
    // `force_visible` recomputes internally, so the visible set is consistent before refresh.
    deps.search
        .borrow_mut()
        .force_visible(added_ids.clone(), &deps.library.borrow());
    refresh_library_carousel(ui, deps, false);
    #[cfg(target_arch = "wasm32")]
    ui.invoke_storage_inventory_changed();
    match save_result {
        Err(e) => {
            tracing::error!(error = %e, "failed to save library after {op}");
            // Save failure is an error the user must see → toast, not the strip.
            set_add_toast(
                ui,
                crate::i18n::dynamic::added_books_save_failed(loader, added_ids.len(), &e),
            );
        }
        Ok(()) => {
            // Some books were added; route through the pure decision fn so the
            // 4-way mapping is testable without Slint.
            match select_add_notice(added_ids.len(), skipped) {
                AddNotice::AddedWithSkips {
                    added: n,
                    skipped: s,
                } => {
                    // Partial add: some images were skipped → warn on the toast.
                    set_add_toast(ui, crate::i18n::dynamic::added_books_skipped(loader, n, s));
                }
                AddNotice::Added { added: n } => {
                    // Clean success → quiet strip.
                    set_status_strip(ui, crate::i18n::dynamic::added_books(loader, n));
                }
                // added_ids is non-empty here, so AlreadyInLibrary and
                // NoneAddedAllSkipped are unreachable; exhaustive for safety.
                _ => set_status_strip(
                    ui,
                    crate::i18n::dynamic::added_books(loader, added_ids.len()),
                ),
            }
        }
    }
    // Focus the first newly added book by its VISIBLE row (the carousel renders
    // the filtered slice), not its full-library index.
    if let Some(first_id) = added_ids.first() {
        let index = {
            let lib = deps.library.borrow();
            let search = deps.search.borrow();
            visible_focus_index_for_id(&lib, search.visible_indices(), first_id)
        };
        if let Some(index) = index {
            ui.set_carousel_focused_index(index as i32);
        } else {
            // force_visible + recompute guarantees the just-added book is a visible row, so
            // this is unreachable; fail loudly in dev/test, log and fall through in release.
            debug_assert!(
                false,
                "add: forced-visible book {} not found in visible rows",
                first_id
            );
            tracing::warn!(
                id = %first_id,
                "add: forced-visible book not found in visible rows; focus not restored"
            );
        }
    }
    ui.invoke_focus_carousel();
}

#[cfg(test)]
mod tests {
    use super::*;
    use slint::Model;

    fn refresh_deps(library: Library, query: &str) -> CarouselRefresh {
        let mut search = LibrarySearchState::default();
        search.set_query(query.into(), &library);
        CarouselRefresh {
            library: Rc::new(RefCell::new(library)),
            library_store: Rc::new(None),
            covers: Rc::new(cover_loader::CoverController::new(
                crate::backend::fake::FakeBackend::new(),
                Rc::new(RefCell::new(gashuu_core::Settings::default())),
            )),
            search: Rc::new(RefCell::new(search)),
            selection: Rc::new(RefCell::new(LibrarySelectionState::default())),
            localizer: Rc::new(i18n::Localizer::new(gashuu_core::Language::En)),
        }
    }

    fn removal_library() -> Library {
        let mut library = Library::new();
        for title in ["four-x", "one-x", "three-x", "two"] {
            assert!(library.add(BookId::new(title), title.into()).is_some());
        }
        library
    }

    fn titles_after_empty_book_removal(deps: &CarouselRefresh) -> Vec<String> {
        let ui = cover_loader::test_window();
        assert!(deps.library.borrow_mut().remove(&BookId::new("four-x")));
        finalize_empty_book_rejected(
            &ui,
            deps,
            &EmptyBookOutcome {
                title: "four-x".into(),
                removed: true,
                save_error: None,
            },
        );
        ui.get_carousel_items()
            .iter()
            .map(|item| item.title.to_string())
            .collect()
    }

    #[test]
    fn prune_selection_to_shelf_drops_only_removed_books() {
        let [a, b, c] = ["A", "B", "C"].map(BookId::new);
        let mut library = Library::new();
        assert!(library.add(a.clone(), "A".into()).is_some());
        assert!(library.add(b.clone(), "B".into()).is_some());
        let library = RefCell::new(library);
        let mut selection = LibrarySelectionState::default();
        selection.insert(a.clone());
        selection.insert(b.clone());
        selection.insert(c);
        let selection = RefCell::new(selection);

        assert_eq!(prune_selection_to_shelf(&selection, &library), 1);
        assert_eq!(selection.borrow().selected().collect::<Vec<_>>(), [&a, &b]);
    }

    #[test]
    fn prune_selection_to_shelf_is_a_no_op_when_consistent() {
        let [a, b] = ["A", "B"].map(BookId::new);
        let mut library = Library::new();
        assert!(library.add(a, "A".into()).is_some());
        assert!(library.add(b.clone(), "B".into()).is_some());
        let library = RefCell::new(library);
        let mut selection = LibrarySelectionState::default();
        selection.insert(b.clone());
        let selection = RefCell::new(selection);

        assert_eq!(prune_selection_to_shelf(&selection, &library), 0);
        assert_eq!(selection.borrow().selected().collect::<Vec<_>>(), [&b]);
    }

    #[test]
    fn empty_book_removal_prunes_the_selection_before_the_toolbar_refresh() {
        let ui = cover_loader::test_window();
        let empty = BookId::new("E");
        let mut library = Library::new();
        assert!(library.add(empty.clone(), "E".into()).is_some());
        assert!(library.add(BookId::new("X"), "X".into()).is_some());
        let deps = refresh_deps(library, "");
        deps.selection.borrow_mut().insert(empty.clone());
        ui.set_carousel_selection_mode(true);
        ui.set_carousel_has_selection(true);

        assert!(deps.library.borrow_mut().remove(&empty));
        finalize_empty_book_rejected(
            &ui,
            &deps,
            &EmptyBookOutcome {
                title: "E".into(),
                removed: true,
                save_error: None,
            },
        );

        assert_eq!(deps.selection.borrow().count(), 0);
        assert!(!ui.get_carousel_has_selection());
        assert!(ui.get_carousel_selection_mode());
    }

    #[test]
    fn stale_removal_leaves_the_selection_untouched() {
        let ui = cover_loader::test_window();
        let stale = BookId::new("E");
        let remaining = BookId::new("X");
        let mut library = Library::new();
        assert!(library.add(remaining.clone(), "X".into()).is_some());
        let deps = refresh_deps(library, "");
        deps.selection.borrow_mut().insert(stale.clone());
        deps.selection.borrow_mut().insert(remaining.clone());

        finalize_empty_book_rejected(
            &ui,
            &deps,
            &EmptyBookOutcome {
                title: "E".into(),
                removed: false,
                save_error: None,
            },
        );

        let selection = deps.selection.borrow();
        assert_eq!(
            selection.selected().collect::<Vec<_>>(),
            [&stale, &remaining]
        );
    }

    #[test]
    fn empty_book_removal_under_a_filter_shows_only_matching_books() {
        let deps = refresh_deps(removal_library(), "x");
        assert_eq!(titles_after_empty_book_removal(&deps), ["one-x", "three-x"]);
    }

    #[test]
    fn empty_book_removal_keeps_forced_visible_books() {
        let deps = refresh_deps(removal_library(), "x");
        deps.search
            .borrow_mut()
            .force_visible([BookId::new("two")], &deps.library.borrow());
        assert_eq!(
            titles_after_empty_book_removal(&deps),
            ["one-x", "three-x", "two"]
        );
    }

    #[test]
    fn empty_book_removal_without_a_filter_lists_every_remaining_book() {
        let deps = refresh_deps(removal_library(), "");
        assert_eq!(
            titles_after_empty_book_removal(&deps),
            ["one-x", "three-x", "two"]
        );
    }

    #[test]
    fn visible_focus_index_for_id_uses_filtered_rows() {
        let mut lib = Library::new();
        assert!(lib
            .add(BookId::new("/manga/alpha.cbz"), "alpha".into())
            .is_some());
        assert!(lib
            .add(BookId::new("/manga/beta.cbz"), "beta".into())
            .is_some());
        assert!(lib
            .add(BookId::new("/manga/gamma.cbz"), "gamma".into())
            .is_some());

        let path = lib.books()[2].id().clone();
        // gamma is the only visible row, so its visible position is 0.
        assert_eq!(visible_focus_index_for_id(&lib, &[2], &path), Some(0));
        // gamma is not among the visible rows, so it has no visible position.
        assert_eq!(visible_focus_index_for_id(&lib, &[0, 1], &path), None);
    }

    // ---- entry_focus_index (continue reading) ----------------------------

    /// Build a 3-book library (alpha/beta/gamma, natural order) and mark `index`
    /// as last-opened via `register_opened`. Returns the library; the natural
    /// order is alpha(0), beta(1), gamma(2) since the paths sort lexically.
    fn library_with_last_opened(index: usize) -> Library {
        let mut lib = Library::new();
        for leaf in ["alpha", "beta", "gamma"] {
            assert!(lib
                .add(BookId::new(format!("/manga/{leaf}.cbz")), leaf.into())
                .is_some());
        }
        let path = lib.books()[index].id().clone();
        lib.register_opened(&path, "", None);
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
            .add(BookId::new("/manga/alpha.cbz"), "alpha".into())
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
}
