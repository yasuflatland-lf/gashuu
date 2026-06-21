//! Library carousel refresh + projection: rebuild/bind the filtered carousel
//! model, (re)start cover loading for the visible rows, and map between visible
//! carousel rows and library book paths.
//!
//! Extracted from `main.rs` (mirrors the `view_sync.rs` split): these items form
//! one cohesive unit — the [`CarouselRefresh`] collaborator bundle and every
//! function that consumes it — driven almost entirely from `handlers::library`
//! and `handlers::settings`. `go_to_library` / `go_to_viewer` stay in `main.rs`
//! and route their carousel work through here. UI-thread only.

use crate::add_books::{select_add_notice, AddNotice, AddReport};
use crate::carousel::{
    apply_selection_flags, bind_carousel_model, build_carousel_model, cover_requests,
};
use crate::library_model::{LibrarySearchState, LibrarySelectionState};
use crate::open_book::EmptyBookRemoval;
use crate::{cover_loader, i18n, ViewerWindow};
use gashuu_core::Library;
use slint::ComponentHandle;
use std::cell::RefCell;
use std::rc::Rc;

/// Resolve a VISIBLE carousel `index` to its underlying library book path,
/// through the search state's projection — the SAME hop `on_carousel_open` uses
/// (the carousel row is an index into `visible_indices`, which maps to a library
/// row). Returns `None` for an out-of-range index or a carousel/library desync.
/// Borrows `library` and `search` only for the duration of the call.
pub(crate) fn visible_index_to_path(
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
pub(crate) fn visible_focus_index_for_path(
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
pub(crate) fn snap_carousel_focus_to_last_opened(
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
pub(crate) fn clamp_focused_index(old: i32, visible_count: usize) -> i32 {
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
pub(crate) fn push_selection_strings(
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
/// `apply_add_report` under the argument-count limit and documents that
/// they are one collaboration unit, not independent params.
pub(crate) struct CarouselRefresh<'a> {
    pub(crate) library: &'a Rc<RefCell<Library>>,
    pub(crate) covers: &'a cover_loader::CoverController,
    pub(crate) search: &'a Rc<RefCell<LibrarySearchState>>,
    pub(crate) selection: &'a Rc<RefCell<LibrarySelectionState>>,
    pub(crate) localizer: &'a Rc<i18n::Localizer>,
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

/// The shared UI tail of an empty-book auto-removal, called by both the open-time
/// `finalize_open` arm and the cover-time `on_empty_book_detected` handler so the
/// two cannot drift. Rebuilds the carousel through the chokepoint (removed book
/// disappears; the cover-epoch bump drops any sibling cover still streaming for
/// it; active filter preserved; no focus reset), then — only when THIS path
/// performed the removal (`removed == true`) — surfaces the auto-removal notice
/// (with the library-save-failure detail appended). The `removed`-idempotency
/// guard is single-sourced here: a `removed == false` (race, or a never-added
/// empty source) rebuilds but stays silent.
pub(crate) fn finalize_empty_book_removed(
    ui: &ViewerWindow,
    deps: &CarouselRefresh,
    removal: &EmptyBookRemoval,
) {
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

/// Apply an already-computed add `report` to the library: persist, rebuild the
/// filtered carousel, and surface the outcome on the status line, restoring
/// carousel focus in every case. The UI-thread tail of the bulk add (issue 206),
/// run from the `add-finalize` handler once the off-thread probe completes (and
/// the `apply_outcomes` mutation has produced the `report`).
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
    report: AddReport,
    op: &'static str,
    loader: &i18n_embed::fluent::FluentLanguageLoader,
) {
    let AddReport {
        added: added_paths,
        skipped,
    } = report;
    if added_paths.is_empty() {
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
            // Save failure is an error the user must see → toast, not the strip.
            set_add_toast(
                ui,
                crate::i18n::dynamic::added_books_save_failed(loader, added_paths.len(), &e),
            );
        }
        Ok(()) => {
            // Some books were added; route through the pure decision fn so the
            // 4-way mapping is testable without Slint.
            match select_add_notice(added_paths.len(), skipped) {
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
                // added_paths is non-empty here, so AlreadyInLibrary and
                // NoneAddedAllSkipped are unreachable; exhaustive for safety.
                _ => set_status_strip(
                    ui,
                    crate::i18n::dynamic::added_books(loader, added_paths.len()),
                ),
            }
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
}
