//! The "remove the selected books" bulk-delete application use case, split out
//! of `app.rs` (#241).
//!
//! The bulk-remove surface here ([`RemoveBooksUseCase`] + [`RemoveOutcome`] +
//! [`remove_books_with_rollback`] + [`removed_contains_open`] + the
//! [`ConfirmDeleteContent`] builder) is consumed by `main.rs` (cluster W) and the
//! Slint `ConfirmDialog` wiring, the destructive-delete handlers in
//! `handlers::library` being the live runtime callers (PR-5 #129). The use case
//! is lean and headless — it returns a [`RemoveOutcome`] and touches no Slint; the
//! UI tail lives in
//! [`finalize_remove`](crate::carousel_refresh::finalize_remove).

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use gashuu_core::{Book, CoreError, Library, RemovalReport, ThumbnailCache};
use i18n_embed::fluent::FluentLanguageLoader;

use crate::cover_loader::purge_cover;
use crate::library_model::{LibrarySearchState, LibrarySelectionState};
use crate::viewer_state::ViewerState;

/// Maximum number of book titles listed verbatim in the delete-confirmation
/// dialog body before the list is truncated with an "…and M more" line. Beyond
/// this the dialog would grow unboundedly for a large bulk selection.
const CONFIRM_DELETE_LIST_CAP: usize = 10;

/// Result of [`RemoveBooksUseCase::run`]. Carries enough for `main.rs` to
/// finalize the UI (status line / carousel rebuild) without any i18n logic here.
#[derive(Debug, PartialEq)]
pub(crate) enum RemoveOutcome {
    /// Nothing was selected — the destructive path is a no-op; the caller should
    /// not even have reached the confirm dialog, but the guard is honest.
    NoSelection,
    /// The library mutated in memory but the persistence save failed. The shelf
    /// has been rolled back byte-identically and the selection is PRESERVED, so
    /// the user can retry. `error` is the pre-captured (untranslated) detail.
    SaveFailed { error: String },
    /// `n` books were removed and saved. `closed_open_book` is true when the book
    /// that was open in the viewer was among them (the viewer was cleared to the
    /// no-book-open state). `n` counts only the books actually removed
    /// (`RemovalReport::removed`), excluding any stale `not_found` paths.
    Removed { n: usize, closed_open_book: bool },
}

/// Remove every book in `paths` from `library` and persist, rolling back the
/// in-memory shelf byte-identically if the save fails.
///
/// The transaction (issue §4):
/// 1. Capture FULL clones of the books about to be removed (so a rollback can
///    re-insert them WITHOUT the `add()`-trap that would reset
///    `resume_page`/`page_count`/`overrides` — see [`Library::restore`]).
/// 2. `remove_many(paths)` drops them in one retain pass and reports the outcome.
/// 3. `save(library)`; on `Err`, `restore` the captured clones (which re-sorts
///    into natural order) and return the error. Caches are NOT touched on
///    failure — only the persisted shelf is the transaction boundary.
///
/// On success the caller purges covers and clears the selection; on failure it
/// must do neither. `save` is injected (`|l| l.save()` in production) so this is
/// unit-testable against an in-memory failing/succeeding save.
pub(crate) fn remove_books_with_rollback(
    library: &mut Library,
    paths: &[PathBuf],
    save: impl FnOnce(&Library) -> Result<(), CoreError>,
) -> Result<RemovalReport, CoreError> {
    // 1. Keep whole-Book clones of the entries we are about to remove, BEFORE the
    //    retain pass drops them, so a failed save can restore them losslessly.
    let removed_books: Vec<Book> = library
        .books()
        .iter()
        .filter(|b| paths.iter().any(|p| p.as_path() == b.path()))
        .cloned()
        .collect();
    // 2. Drop them and report what actually matched (vs. stale not_found inputs).
    let report = library.remove_many(paths);
    // 3. Persist; roll back the in-memory shelf byte-identically on failure.
    match save(library) {
        Ok(()) => Ok(report),
        Err(e) => {
            // `restore` re-inserts the whole clones and re-establishes natural
            // order via the aggregate's own `book_order` — the caller never sorts.
            library.restore(removed_books);
            Err(e)
        }
    }
}

/// Filesystem mtime of `path` as whole seconds since the Unix epoch, or `0` when
/// the file is missing / has no readable mtime. Mirrors the cover cache's key
/// convention (and `cover_loader`'s private `mtime_secs`) so a removed book's strip
/// thumbnails are purged under the SAME key the strip generator wrote them under.
/// Computed inline here rather than imported from `cover_loader` so this strip
/// purge does not force a change to that module's private surface.
fn removed_book_mtime_secs(path: &Path) -> i64 {
    std::fs::metadata(path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Decide whether the currently open file is among the removed paths (so the
/// viewer must be cleared). Pure and testable in isolation from the live
/// `ViewerWindow`, so `RemoveBooksUseCase::run` can stay a thin orchestration
/// shell (like `OpenBookUseCase::run`). `open_file` is `None` when no book is
/// open; comparison is by canonical path identity (the same key the selection
/// and the library store under).
pub(crate) fn removed_contains_open(open_file: Option<&Path>, removed: &[PathBuf]) -> bool {
    open_file.is_some_and(|open| removed.iter().any(|p| p.as_path() == open))
}

/// Coordinates the "remove the selected books" use case. Mirrors
/// [`OpenBookUseCase`](crate::open_book::OpenBookUseCase): the shared
/// collaborators it threads are fields, so the (single) delete-confirm site calls
/// [`run`](RemoveBooksUseCase::run) with no arguments. It stays lean and headless
/// — it touches no Slint at all (only the in-memory `state` / `library` / `search`
/// / `selection`) and returns a [`RemoveOutcome`]. The carousel rebuild / status /
/// focus restoration / viewer title-blank all happen in
/// [`finalize_remove`](crate::carousel_refresh::finalize_remove) from that
/// outcome.
pub(crate) struct RemoveBooksUseCase {
    state: Rc<RefCell<ViewerState>>,
    library: Rc<RefCell<Library>>,
    search: Rc<RefCell<LibrarySearchState>>,
    selection: Rc<RefCell<LibrarySelectionState>>,
}

impl RemoveBooksUseCase {
    pub(crate) fn new(
        state: Rc<RefCell<ViewerState>>,
        library: Rc<RefCell<Library>>,
        search: Rc<RefCell<LibrarySearchState>>,
        selection: Rc<RefCell<LibrarySelectionState>>,
    ) -> Self {
        Self {
            state,
            library,
            search,
            selection,
        }
    }

    /// Execute the destructive transaction in the issue's non-negotiable order:
    /// snapshot the selection → mutate+save with rollback → purge covers (best
    /// effort) → close the viewer's open book if it was deleted → recompute the
    /// search projection → clear the selection (success only).
    ///
    /// Lean and headless: touches no Slint. Returns [`RemoveOutcome::NoSelection`]
    /// for an empty selection, [`RemoveOutcome::SaveFailed`] (selection PRESERVED,
    /// shelf rolled back) when the persistence save fails, and
    /// [`RemoveOutcome::Removed`] otherwise.
    /// [`finalize_remove`](crate::carousel_refresh::finalize_remove) consumes the
    /// outcome to rebuild the carousel, compose the status line, restore focus, and
    /// blank the viewer title (the Slint side of the headless close recorded by
    /// `closed_open_book`).
    pub(crate) fn run(&self) -> RemoveOutcome {
        // 1. Snapshot the selected paths (deterministic BTreeSet order). Empty
        //    selection short-circuits before any mutation.
        let paths: Vec<PathBuf> = self
            .selection
            .borrow()
            .selected()
            .map(Path::to_path_buf)
            .collect();
        if paths.is_empty() {
            return RemoveOutcome::NoSelection;
        }

        // 1b. Snapshot page counts BEFORE removal drops entries so the strip purge can
        //     iterate per-page cache keys; borrow scoped so step 2's mut borrow can't clash.
        let page_counts: HashMap<PathBuf, usize> = {
            let library = self.library.borrow();
            paths
                .iter()
                .filter_map(|path| {
                    library
                        .books()
                        .iter()
                        .find(|b| b.path() == path.as_path())
                        .and_then(Book::page_count_opt)
                        .map(|count| (path.clone(), count))
                })
                .collect()
        };

        // 2. Mutate + save with rollback. The `library.borrow_mut()` is confined
        //    to this statement so the cover-purge borrow below cannot conflict.
        let report = match remove_books_with_rollback(&mut self.library.borrow_mut(), &paths, |l| {
            l.save()
        }) {
            Ok(report) => report,
            Err(e) => {
                tracing::error!(error = %e, "failed to save library on bulk remove; rolled back");
                // SaveFailed preserves the selection (no `selection.clear()`),
                // so the user can retry; the shelf was rolled back in-memory.
                return RemoveOutcome::SaveFailed {
                    error: format!("{e}"),
                };
            }
        };

        // 3. Best-effort purge of each removed book's cover AND its per-page strip
        //    thumbnails; a cache-construction failure skips both purges wholesale.
        match ThumbnailCache::new() {
            Ok(cache) => {
                for path in &report.removed {
                    purge_cover(&cache, path);
                }
                // Strips are keyed PER PAGE, so `purge_cover` (cover key only) orphans
                // them (issue #361); reclaim under the same mtime+max-side the generator used.
                for path in &report.removed {
                    if let Some(&n) = page_counts.get(path) {
                        let mtime = removed_book_mtime_secs(path);
                        cache.purge_pages_for(
                            path,
                            mtime,
                            &[gashuu_core::DEFAULT_THUMB_MAX_SIDE],
                            n,
                        );
                    }
                }
            }
            Err(e) => {
                tracing::warn!(error = %e, "cover cache unavailable; skipping cover purge on remove");
            }
        }

        // 4. If the open book was deleted, close it (no previous book to fall back to).
        //    Headless only; `closed_open_book` tells `finalize_remove` to blank the title.
        let closed_open_book = {
            let open_file = self.state.borrow().open_file().map(Path::to_path_buf);
            if removed_contains_open(open_file.as_deref(), &report.removed) {
                self.state.borrow_mut().close();
                true
            } else {
                false
            }
        };

        // 5. Recompute the visible projection against the shrunken library so the
        //    carousel rebuild in `main.rs` reads a fresh, valid index set.
        self.search.borrow_mut().recompute(&self.library.borrow());

        // 6. Selection is cleared ONLY on success (SaveFailed returned earlier).
        self.selection.borrow_mut().clear();

        RemoveOutcome::Removed {
            n: report.removed.len(),
            closed_open_book,
        }
    }
}

/// Fully localized display content for the bulk-delete confirmation dialog, built by
/// [`confirm_delete_content`] and rendered by the Slint `ConfirmDialog`. Each
/// field is already localized (the builder resolves them via the active loader),
/// but the struct itself carries no i18n logic — it is a plain data bundle so the
/// composition (truncation, the outside-search line, the open-book warning) is
/// unit-testable without a live `ViewerWindow`.
pub(crate) struct ConfirmDeleteContent {
    /// Dialog title, e.g. "Remove 3 books?" — driven by the TOTAL selection count.
    pub title: String,
    /// Up to [`CONFIRM_DELETE_LIST_CAP`] selected book titles (in selection /
    /// `BTreeSet` path order), followed by an "…and M more" line when the
    /// selection exceeds the cap, and finally an "N selected outside the current
    /// search" line when some selected books are not in the visible projection.
    pub body_lines: Vec<String>,
    /// Reassurance that the files on disk are kept (only the library entry goes).
    pub info: String,
    /// Warning shown when the currently OPEN book is among the selection (it will
    /// be closed). Empty string when no open book is selected.
    pub warning: String,
}

/// Build the [`ConfirmDeleteContent`] for the current selection. Pure (no I/O,
/// no Slint) and fully testable.
///
/// - `title` uses the TOTAL selection count (`selection.count()`).
/// - `body_lines` lists up to [`CONFIRM_DELETE_LIST_CAP`] resolvable titles in
///   selection order. A selected path with no library entry (projection drift)
///   is skipped from the LIST but still counts toward the title/`count()` — we
///   stay honest: the title reflects what the user selected, the list reflects
///   what we can name.
/// - When `count > CAP`, an "…and M more" line is appended (`M = count - CAP`).
/// - When some selected books are outside the visible search projection
///   (`count > visible_selected_count`), an "N selected outside the current
///   search" line is appended LAST so the user is not surprised that filtered-out
///   books are deleted too.
/// - `warning` is non-empty only when `open_file` is selected.
pub(crate) fn confirm_delete_content(
    loader: &FluentLanguageLoader,
    selection: &LibrarySelectionState,
    search: &LibrarySearchState,
    library: &Library,
    open_file: Option<&Path>,
) -> ConfirmDeleteContent {
    let count = selection.count();

    // List up to CAP resolvable titles in selection order; a path absent from the
    // library is skipped from the LIST but still counts toward `count` (see fn doc).
    let mut body_lines: Vec<String> = selection
        .selected()
        .filter_map(|path| library.books().iter().find(|b| b.path() == path))
        .take(CONFIRM_DELETE_LIST_CAP)
        .map(|book| book.title().to_string())
        .collect();

    // "…and M more" when the TOTAL selection exceeds the listed cap.
    if count > CONFIRM_DELETE_LIST_CAP {
        body_lines.push(crate::i18n::dynamic::confirm_delete_more(
            loader,
            count - CONFIRM_DELETE_LIST_CAP,
        ));
    }

    // "N selected outside the current search" as the LAST body line, when some
    // selected books are not in the visible projection.
    let visible_selected =
        crate::selection_projection::visible_selected_count(selection, search, library);
    if count > visible_selected {
        body_lines.push(crate::i18n::dynamic::confirm_delete_outside_search(
            loader,
            count - visible_selected,
        ));
    }

    // The open-book warning fires only when the open file is itself selected.
    let warning = match open_file {
        Some(open) if selection.contains(open) => {
            crate::i18n::dynamic::confirm_delete_open_book(loader)
        }
        _ => String::new(),
    };

    ConfirmDeleteContent {
        title: crate::i18n::dynamic::confirm_delete_title(loader, count),
        body_lines,
        info: crate::i18n::dynamic::confirm_delete_keep_files(loader),
        warning,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::num::NonZeroUsize;

    /// Build a real failing `CoreError` for the save-result arguments. Uses the
    /// `Io` variant (the simplest existing variant; `CoreError: From<io::Error>`),
    /// constructed from an `std::io::Error`. No core changes — just construction.
    fn err() -> CoreError {
        std::io::Error::other("x").into()
    }

    // ---- remove_books_with_rollback ---------------------------------------

    /// Build a library with three books, the middle one carrying a non-default
    /// reading position so the `add()`-trap (which resets resume_page to 0) would
    /// surface in a byte-comparison rollback test.
    fn lib_with_three() -> Library {
        let mut lib = Library::new();
        for name in ["a.cbz", "b.cbz", "c.cbz"] {
            assert!(lib.add(PathBuf::from(format!("/manga/{name}"))).is_some());
        }
        assert!(lib.set_resume_page(Path::new("/manga/b.cbz"), 17));
        assert!(lib.set_page_count(Path::new("/manga/b.cbz"), NonZeroUsize::new(80).unwrap()));
        lib
    }

    #[test]
    fn rollback_restores_library_byte_identically_on_save_failure() {
        // The rollback path's whole point: a failed save must leave the shelf byte-
        // identical, even for non-default resume_page/page_count the add()-trap would reset.
        let mut lib = lib_with_three();
        let before = lib.to_json().unwrap();

        let result = remove_books_with_rollback(
            &mut lib,
            &[PathBuf::from("/manga/b.cbz"), PathBuf::from("/manga/c.cbz")],
            |_| Err(err()),
        );

        assert!(result.is_err(), "a failing save must return Err");
        assert_eq!(
            lib.to_json().unwrap(),
            before,
            "the shelf must be byte-identical after a rolled-back save"
        );
        assert_eq!(lib.books().len(), 3, "all books restored");
        assert_eq!(
            lib.resume_page(Path::new("/manga/b.cbz")),
            17,
            "restored book keeps its resume_page (add() would reset it to 0)"
        );
    }

    #[test]
    fn success_path_returns_report_and_keeps_removal() {
        // On a successful save the books stay removed and the report names them.
        let mut lib = lib_with_three();
        let report = remove_books_with_rollback(
            &mut lib,
            &[PathBuf::from("/manga/a.cbz"), PathBuf::from("/manga/c.cbz")],
            |_| Ok(()),
        )
        .expect("a succeeding save returns Ok(report)");

        assert_eq!(report.removed.len(), 2, "two books reported removed");
        assert!(report.not_found.is_empty());
        let titles: Vec<&str> = lib.books().iter().map(|b| b.title()).collect();
        assert_eq!(titles, vec!["b"], "only the unremoved book survives");
    }

    #[test]
    fn report_excludes_not_found_paths_from_the_removed_count() {
        // A stale path (no stored book) is reported not_found, NOT among removed,
        // so the user-facing "removed N" never over-counts a path already gone.
        let mut lib = lib_with_three();
        let report = remove_books_with_rollback(
            &mut lib,
            &[
                PathBuf::from("/manga/a.cbz"),
                PathBuf::from("/manga/ghost.cbz"),
            ],
            |_| Ok(()),
        )
        .expect("succeeding save");

        assert_eq!(report.removed, vec![PathBuf::from("/manga/a.cbz")]);
        assert_eq!(
            report.not_found,
            vec![PathBuf::from("/manga/ghost.cbz")],
            "the stale path is not_found, excluded from the removed count"
        );
    }

    // ---- removed_contains_open (open-book-close decision table) ------------

    #[test]
    fn open_book_in_removed_decides_close() {
        let removed = vec![PathBuf::from("/manga/a.cbz"), PathBuf::from("/manga/b.cbz")];
        assert!(
            removed_contains_open(Some(Path::new("/manga/b.cbz")), &removed),
            "an open book among the removed paths triggers a close"
        );
    }

    #[test]
    fn open_book_not_in_removed_keeps_viewer() {
        let removed = vec![PathBuf::from("/manga/a.cbz")];
        assert!(
            !removed_contains_open(Some(Path::new("/manga/keep.cbz")), &removed),
            "an open book outside the removed set must not be closed"
        );
    }

    #[test]
    fn nothing_open_never_closes() {
        let removed = vec![PathBuf::from("/manga/a.cbz")];
        assert!(
            !removed_contains_open(None, &removed),
            "no open book means there is nothing to close"
        );
    }

    // These build a real En loader and assert STRUCTURALLY (line counts, count digits)
    // rather than byte-exact strings, to avoid coupling to the i18n functions' wording.

    use gashuu_core::Language;

    /// A library + search + selection fixture with `n` books named `book00..`,
    /// all visible under the empty query. Returns the pieces the builder needs.
    fn delete_fixture(n: usize) -> (Library, LibrarySearchState, LibrarySelectionState) {
        let mut lib = Library::new();
        for i in 0..n {
            assert!(lib
                .add(PathBuf::from(format!("/manga/book{i:02}.cbz")))
                .is_some());
        }
        let mut search = LibrarySearchState::default();
        search.set_query(String::new(), &lib);
        let selection = LibrarySelectionState::default();
        (lib, search, selection)
    }

    fn en_loader() -> crate::i18n::Localizer {
        crate::i18n::Localizer::new(Language::En)
    }

    #[test]
    fn confirm_content_lists_titles_and_title_carries_count() {
        let (lib, search, mut sel) = delete_fixture(3);
        for i in 0..3 {
            sel.toggle(PathBuf::from(format!("/manga/book{i:02}.cbz")));
        }
        let loc = en_loader();
        let content = confirm_delete_content(loc.loader(), &sel, &search, &lib, None);

        // Three resolvable titles, no truncation, no outside-search line.
        assert_eq!(content.body_lines.len(), 3, "all three titles listed");
        assert!(
            content.title.contains('3'),
            "title must carry the selection count, got {:?}",
            content.title
        );
        assert!(
            !content.info.is_empty(),
            "keep-files info is always present"
        );
        assert!(content.warning.is_empty(), "no open book ⇒ no warning");
    }

    #[test]
    fn confirm_content_truncates_beyond_cap_with_more_line() {
        // 12 selected ⇒ 10 titles + 1 "…and 2 more" line = 11 body lines.
        let (lib, search, mut sel) = delete_fixture(12);
        for i in 0..12 {
            sel.toggle(PathBuf::from(format!("/manga/book{i:02}.cbz")));
        }
        let loc = en_loader();
        let content = confirm_delete_content(loc.loader(), &sel, &search, &lib, None);

        assert_eq!(
            content.body_lines.len(),
            CONFIRM_DELETE_LIST_CAP + 1,
            "10 titles + 1 'and M more' line"
        );
        let more = content.body_lines.last().unwrap();
        assert!(
            more.contains('2'),
            "the 'and M more' line must carry M = count - cap = 2, got {more:?}"
        );
        assert!(content.title.contains("12"), "title carries the full count");
    }

    #[test]
    fn confirm_content_both_truncation_and_outside_search_lines_appended() {
        // Narrow the query so only book00 is visible but select all 12 (M=2, N=11 distinct).
        // Ordering contract: "and M more" ALWAYS precedes the outside-search line.
        let (lib, mut search, mut sel) = delete_fixture(12);
        // Only "book00" contains the literal "book00" as a substring (book01 etc.
        // do not); the query is an exact substring match, case-insensitive.
        search.set_query("book00".to_string(), &lib);
        for i in 0..12 {
            sel.toggle(PathBuf::from(format!("/manga/book{i:02}.cbz")));
        }
        let loc = en_loader();
        let content = confirm_delete_content(loc.loader(), &sel, &search, &lib, None);

        assert_eq!(
            content.body_lines.len(),
            CONFIRM_DELETE_LIST_CAP + 2,
            "CAP titles + 'and M more' + outside-search line, got {:?}",
            content.body_lines
        );
        // Index CAP must be the "…and M more" line (M = 12 - 10 = 2).
        let more_line = &content.body_lines[CONFIRM_DELETE_LIST_CAP];
        assert!(
            more_line.contains('2'),
            "'and M more' line at index CAP must carry M = 2, got {more_line:?}"
        );
        // Index CAP+1 must be the outside-search line (12 - 1 visible = 11).
        let outside_line = &content.body_lines[CONFIRM_DELETE_LIST_CAP + 1];
        assert!(
            outside_line.contains("11"),
            "outside-search line at index CAP+1 must carry 11, got {outside_line:?}"
        );
        assert!(
            content.title.contains("12"),
            "title carries the full selection count, got {:?}",
            content.title
        );
    }

    #[test]
    fn confirm_content_appends_outside_search_line() {
        // 3 books; narrow the query so only one is visible, but select all three.
        // count(3) > visible_selected(1) ⇒ an outside-search line is appended.
        let (lib, mut search, mut sel) = delete_fixture(3);
        // Only book00 matches this query.
        search.set_query("book00".to_string(), &lib);
        for i in 0..3 {
            sel.toggle(PathBuf::from(format!("/manga/book{i:02}.cbz")));
        }
        let loc = en_loader();
        let content = confirm_delete_content(loc.loader(), &sel, &search, &lib, None);

        // 3 titles + 1 outside-search line (count - visible_selected = 3 - 1 = 2).
        assert_eq!(
            content.body_lines.len(),
            4,
            "3 titles + outside-search line"
        );
        let last = content.body_lines.last().unwrap();
        assert!(
            last.contains('2'),
            "outside-search line must carry 2 (count - visible_selected), got {last:?}"
        );
    }

    #[test]
    fn confirm_content_no_outside_search_line_when_all_visible() {
        // All selected books are visible ⇒ no outside-search line.
        let (lib, search, mut sel) = delete_fixture(2);
        for i in 0..2 {
            sel.toggle(PathBuf::from(format!("/manga/book{i:02}.cbz")));
        }
        let loc = en_loader();
        let content = confirm_delete_content(loc.loader(), &sel, &search, &lib, None);
        assert_eq!(
            content.body_lines.len(),
            2,
            "no outside-search line when the whole selection is visible"
        );
    }

    #[test]
    fn confirm_content_warns_when_open_book_is_selected() {
        let (lib, search, mut sel) = delete_fixture(2);
        let open = PathBuf::from("/manga/book00.cbz");
        sel.toggle(open.clone());
        let loc = en_loader();
        let content = confirm_delete_content(loc.loader(), &sel, &search, &lib, Some(&open));
        assert!(
            !content.warning.is_empty(),
            "the open book being selected must produce a non-empty warning"
        );
    }

    #[test]
    fn confirm_content_no_warning_when_open_book_not_selected() {
        let (lib, search, mut sel) = delete_fixture(2);
        // Select book01, but the open book is book00 (not selected).
        sel.toggle(PathBuf::from("/manga/book01.cbz"));
        let open = PathBuf::from("/manga/book00.cbz");
        let loc = en_loader();
        let content = confirm_delete_content(loc.loader(), &sel, &search, &lib, Some(&open));
        assert!(
            content.warning.is_empty(),
            "an unselected open book must not produce a warning"
        );
    }

    #[test]
    fn confirm_content_skips_unresolvable_path_from_list_but_counts_it_in_title() {
        // Projection drift: a path with no library entry is skipped from the LIST but
        // still counts (honesty rule), and being outside search adds an outside-search line.
        let (lib, search, mut sel) = delete_fixture(1);
        sel.toggle(PathBuf::from("/manga/book00.cbz")); // resolvable
        sel.toggle(PathBuf::from("/manga/ghost.cbz")); // not in library
        let loc = en_loader();
        let content = confirm_delete_content(loc.loader(), &sel, &search, &lib, None);

        assert_eq!(
            content.body_lines.len(),
            2,
            "exactly one title line + one outside-search line, got {:?}",
            content.body_lines
        );
        assert_eq!(
            content.body_lines.first().map(String::as_str),
            Some("book00"),
            "the resolvable title leads the body, got {:?}",
            content.body_lines
        );
        assert!(
            !content.body_lines.iter().any(|l| l.contains("ghost")),
            "the unresolvable path must not appear as a title line, got {:?}",
            content.body_lines
        );
        // But it still counts toward the title (count == 2, honesty rule).
        assert!(
            content.title.contains('2'),
            "title still counts the unresolvable selection, got {:?}",
            content.title
        );
    }
}
