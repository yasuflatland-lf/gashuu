//! The "open a book" application use case, extracted out of `main.rs`.
//!
//! [`OpenBookUseCase`] bundles the seven shared collaborators the open path
//! coordinates (state, settings, viewport, library, thumbs, covers, search) as
//! fields, so the open sites call [`OpenBookUseCase::run`] with just the per-call
//! `ui`, `path`, and `skipped_detail` instead of threading a nine-argument free
//! fn under `#[allow(clippy::too_many_arguments)]`. It touches Slint (status
//! text, carousel rebuild, thumbnail launch), so it lives in the UI crate.

use std::cell::RefCell;
use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use gashuu_core::{Book, CoreError, Library, RemovalReport, Settings, ThumbnailCache};
use i18n_embed::fluent::FluentLanguageLoader;
use slint::ComponentHandle;

use crate::carousel::{bind_carousel_model, build_carousel_model, cover_requests};
use crate::cover_loader::{mtime_secs, CoverController, COVER_MAX_SIDE};
use crate::library_model::{LibrarySearchState, LibrarySelectionState};
use crate::thumbnail_strip::ThumbnailController;
use crate::viewer_state::ViewerState;
use crate::viewport::ViewportState;
use crate::{persist_view_modes, write_back_position, ViewModeRoute, ViewerWindow};

/// Neutral content description of notices to append to the status line after
/// an open. No i18n; all string formatting happens in `i18n::dynamic`.
///
/// `settings_save_err` is `None` if no save was attempted (tracking off) or
/// if the save succeeded. `library_save_err` is `None` on success.
/// Error details are pre-captured as `String` (via `Display`) so no
/// `CoreError` needs to escape this module.
#[derive(Debug, PartialEq)]
pub(crate) struct NoticesContent {
    pub(crate) skipped: usize,
    pub(crate) skipped_detail: SkippedDetail,
    pub(crate) settings_save_err: Option<String>,
    pub(crate) library_save_err: Option<String>,
}

/// Result of [`OpenBookUseCase::run`]. Carries enough information for
/// `main.rs` to finalize the UI without any i18n logic in this module.
#[derive(Debug)]
pub(crate) enum OpenOutcome {
    /// The open failed with this pre-captured error detail (untranslated).
    /// `main.rs` wraps it in `i18n::dynamic::open_error_str(loader, &e_str)`.
    Error(String),
    /// The open succeeded. `main.rs` should call `refresh()` and then
    /// `i18n::dynamic::format_notices(loader, &notices)` to finalize status.
    Success(NoticesContent),
    /// The source opened cleanly but has zero pages, so the book must NOT be
    /// entered in the viewer. The book has been removed from the library (if it
    /// was present) and the library re-saved. `main.rs` stays on the Library
    /// screen, rebuilds the carousel, and shows a notice built from this data.
    ///
    /// `title` is the removed book's display title (looked up from the stored
    /// `Book` when present, otherwise derived from the path the same way `Book`
    /// derives it). `removed` is whether the book was actually in the library
    /// (false when opening a never-added empty source). `save_error` carries the
    /// pre-captured (untranslated) library-save error, `None` when the save
    /// succeeded or no save was attempted (nothing removed).
    EmptyBookRemoved {
        title: String,
        removed: bool,
        save_error: Option<String>,
    },
}

/// Which "entries skipped" detail suffix the open path appends to the skipped
/// notice: folder opens add nothing; archive opens name the skip reasons
/// (zip-slip / oversized entries). Carried as data (not a pre-formatted string)
/// so the formatting layer can render it in the active UI language.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SkippedDetail {
    None,
    Archive,
}

/// Coordinates the "open a book" use case. The collaborators it threads are
/// fields, so the open sites call [`OpenBookUseCase::run`] instead of passing
/// nine arguments through a free fn.
pub(crate) struct OpenBookUseCase {
    state: Rc<RefCell<ViewerState>>,
    settings: Rc<RefCell<Settings>>,
    viewport: Rc<RefCell<ViewportState>>,
    library: Rc<RefCell<Library>>,
    thumbs: Rc<ThumbnailController>,
    covers: Rc<CoverController>,
    /// Shared library-search filter, so the open-time page-count rebuild below
    /// preserves the active filter instead of rebuilding the full library.
    search: Rc<RefCell<LibrarySearchState>>,
}

impl OpenBookUseCase {
    pub(crate) fn new(
        state: Rc<RefCell<ViewerState>>,
        settings: Rc<RefCell<Settings>>,
        viewport: Rc<RefCell<ViewportState>>,
        library: Rc<RefCell<Library>>,
        thumbs: Rc<ThumbnailController>,
        covers: Rc<CoverController>,
        search: Rc<RefCell<LibrarySearchState>>,
    ) -> Self {
        Self {
            state,
            settings,
            viewport,
            library,
            thumbs,
            covers,
            search,
        }
    }

    /// Open `path` and present it: write back the previous position, open the
    /// source, reconcile + save settings (when recent-files tracking is on),
    /// register + save the library, rebuild the carousel, and launch thumbnails.
    ///
    /// Returns [`OpenOutcome::Error`] with a pre-captured error string on failure,
    /// or [`OpenOutcome::Success`] with neutral [`NoticesContent`] on success.
    /// The caller (`main.rs`) is responsible for calling `refresh()` and
    /// formatting notices via `i18n::dynamic`.
    ///
    /// `skipped_detail` is [`SkippedDetail::None`] for folders and
    /// [`SkippedDetail::Archive`] for archives.
    pub(crate) fn run(
        &self,
        ui: &ViewerWindow,
        path: &Path,
        skipped_detail: SkippedDetail,
    ) -> OpenOutcome {
        // Alias the fields so the moved body reads identically at its call
        // sites. In the old free fn `thumbs`/`covers` were `&ThumbnailController`
        // / `&CoverController`; here they are `&Rc<ThumbnailController>` /
        // `&Rc<CoverController>`. `.start()` resolves through the extra `Rc`
        // `Deref` transparently, so no statement in the body needed changing.
        let state = &self.state;
        let settings = &self.settings;
        let viewport = &self.viewport;
        let library = &self.library;
        let thumbs = &self.thumbs;
        let covers = &self.covers;

        // Write back the position for the book that is currently open (if any)
        // before we replace the source. `open_file()` is None when no book was
        // open, so write_back_position is a no-op in that case.
        write_back_position(state, library);
        // Also capture the OUTGOING book's runtime view modes into its per-book
        // override before the source is replaced, so a bare D/R/C/fit toggle
        // persists even when the settings dialog was never opened. No-op when no
        // book is open. View-mode routing goes through `persist_view_modes`
        // (ADR-0007 clobber-trap); `settings` is unread on this per-book route.
        persist_view_modes(
            ViewModeRoute::OpenDifferentBook,
            state,
            viewport,
            settings,
            library,
        );
        // Bind the result first so the `state.borrow_mut()` temporary drops before the
        // `Ok` arm reads `state` again (a borrow held across the match would
        // double-borrow-panic at the `canonical = state.borrow().open_file()...` read
        // below).
        let opened = state.borrow_mut().open_folder(path);
        // Discriminate the open result only — the recents push + settings save are
        // DEFERRED until after the empty-book check below, so a zero-page source
        // pushes nothing to recents and triggers no settings save (the spec pins
        // these side effects as bypassed for an empty book). For a non-empty book
        // the deferred save runs at the original point in the flow, preserving the
        // exact behavior (and the AFTER-refresh surfacing of its outcome).
        match opened {
            Ok(()) => {
                tracing::info!(path = %path.display(), "opened source");
            }
            Err(e) => {
                tracing::error!(error = %e, "failed to open source");
                return OpenOutcome::Error(format!("{e}"));
            }
        }
        // The CANONICAL key the source was opened under (read from `open_file`),
        // not the raw dialog `path`, which may be a non-canonical dialog path. It is
        // the same key `last_page`/`set_page_count`/`write_back_position` use. The
        // `state.borrow()` `Ref` drops at the `;`.
        let canonical = state.borrow().open_file().map(Path::to_path_buf);
        // `page_count()` returns a `Copy` `usize`; the `state.borrow()` drops at the
        // `;` so it cannot conflict with the `library.borrow_mut()` below.
        let page_count = state.borrow().page_count();
        // Reject an empty book: the source opened cleanly but has zero pages, so it
        // must NOT be entered in the viewer. Bail out HERE, before `register_opened`,
        // so the bypassed side effects pinned by the spec never run — no recents
        // push, no settings save (deferred below), no `register_opened`, no per-book
        // view resolve, no library-save-on-open, no carousel rebuild, no thumbnail
        // start, no cover purge (cover purge happens only in the cover-load path, by
        // design). When `canonical` is None we cannot identify the book, so we keep
        // the existing behavior and fall through to the warning branch below.
        //
        // Reviewer note: `ViewerState` still holds this empty source after the
        // bail-out; that is inert (page_count == 0, the viewer is never shown) and
        // the next successful open replaces it — by design, with no restore machinery.
        if page_count == 0 {
            if let Some(c) = canonical.as_deref() {
                // Prefer the stored Book's title; fall back to the path-derived
                // title (matching `Book`'s derivation) when the book was never added.
                let title = library
                    .borrow()
                    .books()
                    .iter()
                    .find(|b| b.path() == c)
                    .map(|b| b.title().to_string())
                    .unwrap_or_else(|| derive_title(c));
                // `Library::remove` is idempotent and returns false when the book is
                // absent; it also clears `last_opened` when it pointed at this book.
                let removed = library.borrow_mut().remove(c);
                // Persist only when something was actually removed; otherwise the
                // shelf is unchanged so there is nothing to save.
                let save_error = if removed {
                    match library.borrow().save() {
                        Ok(()) => None,
                        Err(e) => {
                            tracing::error!(error = %e, "failed to save library after removing empty book");
                            Some(format!("{e}"))
                        }
                    }
                } else {
                    None
                };
                return OpenOutcome::EmptyBookRemoved {
                    title,
                    removed,
                    save_error,
                };
            }
        }
        // The non-empty path resumes here. Run the DEFERRED recents push + settings
        // save now that we know the book has pages — this is the same persistence
        // the open-Ok arm used to do inline, moved past the empty-book bail-out so it
        // is bypassed for a zero-page source. Behavior for a non-empty book is
        // unchanged: `None` when recents tracking is off, `Some(result)` otherwise,
        // surfaced AFTER refresh (composing onto the status line). We intentionally
        // do NOT reconcile the runtime view modes into Settings here — the runtime
        // currently holds the just-opened/outgoing book's per-book modes, not the
        // global defaults; global view modes change only via the Library settings
        // dialog and the no-book-open exit path. This save writes the recents list +
        // cache/preload/track plus the UNCHANGED global view-mode fields.
        let settings_save: Option<Result<(), CoreError>> = {
            let mut s = settings.borrow_mut();
            if s.track_recent_files {
                s.push_recent(path.to_path_buf());
                let result = s.save();
                if let Err(e) = &result {
                    tracing::error!(error = %e, "failed to save settings on open");
                }
                Some(result)
            } else {
                None
            }
        };
        // `register_opened` performs the idempotent add, the page-count back-fill,
        // and the resume lookup as one domain rule. The unknown total is carried by
        // the type: `NonZeroUsize::new(page_count)`. A zero-page open never reaches
        // here (the empty-book bail-out above returned for it), so this is always
        // `Some(page_count)` in practice; the `NonZeroUsize` wrapping keeps the type
        // honest without a `> 0` guard / `debug_assert` at this call site. Borrow
        // discipline: it holds
        // `library.borrow_mut()` only for the `let reg = ...` line (released at its
        // `;`, before the `jump_to` below); `state.borrow_mut().jump_to(...)` is a
        // separate statement on a distinct `RefCell`.
        let count_changed = if let Some(c) = canonical.as_deref() {
            let reg = library
                .borrow_mut()
                .register_opened(c, NonZeroUsize::new(page_count));
            // Resume at the recorded position; for a never-read book `reached` is 0
            // and `jump_to(0)` is a no-op when the index is already 0.
            state.borrow_mut().jump_to(reg.resume.reached());
            reg.count_changed
        } else {
            // Unreachable in practice: a successful open always sets `open_file`.
            // Log if the invariant ever breaks so the book-not-registered failure
            // (no saved reading position) is debuggable instead of silent.
            tracing::warn!(
                path = %path.display(),
                "open_file was None after a successful open; book not registered in library"
            );
            false
        };
        // Resolve THIS book's per-book override against the global defaults and
        // apply it to the runtime BEFORE the first refresh, so the book opens with
        // its own modes (empty override => the global defaults). This runs after
        // `jump_to` above, so the resumed page is then re-anchored to a valid
        // spread leading for the applied spread/cover modes.
        //
        // Borrow discipline: the `library`/`settings` shared borrows are confined
        // to the block expression and drop at its `}`; `resolved` is `Copy`, so the
        // `state`/`viewport` `borrow_mut()`s below hold no other borrow.
        let resolved = {
            let overrides = canonical
                .as_deref()
                .map(|c| library.borrow().overrides_for(c))
                .unwrap_or_default();
            overrides.resolve(&settings.borrow())
        };
        state.borrow_mut().apply_resolved_view(resolved);
        viewport.borrow_mut().set_fit(resolved.fit_mode);
        // Persist the newly registered book (and any back-filled page count)
        // immediately, mirroring the recents save-on-open above, so the library
        // shelf stays consistent even if the app exits before the next leave point.
        // Capture the result to surface AFTER refresh (surfacing now would be
        // clobbered by the spread/status push). Borrow discipline: `register_opened`'s
        // borrow_mut dropped at its `;`; this is a fresh borrow.
        let library_save: Result<(), CoreError> = library.borrow().save();
        if let Err(e) = &library_save {
            tracing::error!(error = %e, "failed to save library on open");
        }
        // When the stored page count was just back-filled/updated, rebuild the
        // carousel model so the home screen reflects the real total/progress when
        // the user returns to the library. Preserve the ACTIVE filter: recompute
        // the shared search state and rebuild from its visible indices (not the
        // full library), so an open-time backfill never resurrects filtered-out
        // books. The carousel's focused index is a separate Slint property and is
        // intentionally NOT reset here — this is a page-count refresh, not a filter
        // update. The `library.borrow()` drops before `covers.start`'s `borrow_mut`.
        if count_changed {
            // Recompute the filter against the changed library, then build the
            // model and cover requests under one borrow that drops before the bind
            // and `covers.start`. The rebuild resets each visible row's cover to a
            // placeholder; re-streaming repaints hits and regenerates misses, and
            // the epoch bump supersedes any covers still streaming from the old model.
            let (model, cover_reqs) = {
                let lib = library.borrow();
                let mut search = self.search.borrow_mut();
                search.recompute(&lib);
                let indices = search.visible_indices();
                (
                    build_carousel_model(&lib, indices),
                    cover_requests(&lib, indices),
                )
            };
            bind_carousel_model(ui, model);
            covers.start(ui.as_weak(), library, cover_reqs);
        }
        // Kick off parallel thumbnail generation for the newly opened source.
        thumbs.start(
            ui.as_weak(),
            state.borrow().current_source(),
            state.borrow().page_count(),
        );
        let skipped = state.borrow().last_open_skipped();
        OpenOutcome::Success(notices_content(
            skipped,
            skipped_detail,
            settings_save.as_ref(),
            &library_save,
        ))
    }
}

/// Collect neutral open-result notices without i18n. Formatting is deferred
/// to `i18n::dynamic::format_notices`.
///
/// Order: skipped entries → settings-save failure → library-save failure.
/// `None` `settings_save` means tracking was off (no save attempted).
pub(crate) fn notices_content(
    skipped: usize,
    skipped_detail: SkippedDetail,
    settings_save: Option<&Result<(), CoreError>>,
    library_save: &Result<(), CoreError>,
) -> NoticesContent {
    NoticesContent {
        skipped,
        skipped_detail,
        settings_save_err: settings_save.and_then(|r| r.as_ref().err().map(|e| format!("{e}"))),
        library_save_err: library_save.as_ref().err().map(|e| format!("{e}")),
    }
}

/// Derive a display title from a path, mirroring `gashuu_core::Book`'s own
/// derivation: the directory name for a folder, the file stem for a file, and
/// the lossy full path string as a last-resort fallback so the title is never
/// empty. Used by the empty-book reject path only when the source was never
/// added to the library (so no stored `Book::title` exists to reuse). `Book`'s
/// derivation is `pub(crate)` to `gashuu-core` and therefore not reachable from
/// this crate, so the logic is replicated here rather than imported.
fn derive_title(path: &Path) -> String {
    if path.is_dir() {
        path.file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .filter(|s| !s.is_empty())
    } else {
        path.file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .filter(|s| !s.is_empty())
    }
    .unwrap_or_else(|| path.to_string_lossy().into_owned())
}

// The bulk-remove surface below (RemoveBooksUseCase + RemoveOutcome +
// remove_books_with_rollback + removed_contains_open + the ConfirmDeleteContent
// builder) is consumed by `main.rs` (cluster W) and the Slint ConfirmDialog
// wiring, the destructive-delete handlers in `main.rs` being the live runtime
// callers (PR-5 #129).

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
///    `last_page`/`page_count`/`overrides` — see [`Library::restore`]).
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
/// [`OpenBookUseCase`]: the shared collaborators it threads are fields, so the
/// (single) delete-confirm site calls [`run`](RemoveBooksUseCase::run) with just
/// the `ui`. It touches Slint (clears the viewer title on a closed open book),
/// so it lives in the UI crate. Carousel rebuild / status / focus restoration
/// after a successful removal happen in `main.rs` from the returned
/// [`RemoveOutcome`].
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
    /// effort) → clear the viewer if the open book was deleted → recompute the
    /// search projection → clear the selection (success only).
    ///
    /// Returns [`RemoveOutcome::NoSelection`] for an empty selection,
    /// [`RemoveOutcome::SaveFailed`] (selection PRESERVED, shelf rolled back) when
    /// the persistence save fails, and [`RemoveOutcome::Removed`] otherwise. The
    /// caller (`main.rs`) rebuilds the carousel and composes the status line.
    pub(crate) fn run(&self, ui: &ViewerWindow) -> RemoveOutcome {
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

        // 3. Best-effort purge of each removed book's persistent cover. mtime
        //    drift is EXPECTED (the file may be gone, or its mtime changed since
        //    the cover was generated) and never surfaced to the user — only
        //    `tracing::warn!`. A cache-construction failure skips the purge wholesale.
        match ThumbnailCache::new() {
            Ok(cache) => {
                for path in &report.removed {
                    let removed = cache.purge_for(path, mtime_secs(path), &[COVER_MAX_SIDE]);
                    if removed == 0 {
                        // Not an error: a missing file / drifted mtime / unwritable
                        // cache entry leaves a harmless orphan the startup prune
                        // sweep reclaims later (issue 143).
                        tracing::warn!(
                            path = %path.display(),
                            "no persistent cover purged for removed book (missing, mtime drift, or unwritable cache)"
                        );
                    }
                }
            }
            Err(e) => {
                tracing::warn!(error = %e, "cover cache unavailable; skipping cover purge on remove");
            }
        }

        // 4. If the open book was deleted, clear the viewer to the no-book-open
        //    state (there is no previous book to fall back to) and blank the
        //    centered title-bar name.
        let closed_open_book = {
            let open_file = self.state.borrow().open_file().map(Path::to_path_buf);
            if removed_contains_open(open_file.as_deref(), &report.removed) {
                self.state.borrow_mut().close();
                ui.set_current_book_name("".into());
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

    // List up to CAP resolvable titles in selection (BTreeSet path) order. A
    // selected path absent from the library is skipped from the LIST (it still
    // counts toward `count` for the title — see the fn doc).
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
    let visible_selected = selection.visible_selected_count(search, library);
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

    /// Build a real failing `CoreError` for the save-result arguments. Uses the
    /// `Io` variant (the simplest existing variant; `CoreError: From<io::Error>`),
    /// constructed from an `std::io::Error`. No core changes — just construction.
    fn err() -> CoreError {
        std::io::Error::other("x").into()
    }

    #[test]
    fn clean_open_emits_no_notices() {
        let c = notices_content(0, SkippedDetail::None, None, &Ok(()));
        assert_eq!(c.skipped, 0);
        assert!(c.settings_save_err.is_none());
        assert!(c.library_save_err.is_none());
        let c2 = notices_content(0, SkippedDetail::Archive, None, &Ok(()));
        assert_eq!(c2.skipped, 0);
    }

    #[test]
    fn skipped_only_emits_skipped_notice() {
        let c = notices_content(3, SkippedDetail::Archive, Some(&Ok(())), &Ok(()));
        assert_eq!(c.skipped, 3);
        assert_eq!(c.skipped_detail, SkippedDetail::Archive);
        assert!(c.settings_save_err.is_none());
        assert!(c.library_save_err.is_none());
    }

    #[test]
    fn settings_failure_only_emits_settings_notice() {
        let c = notices_content(0, SkippedDetail::None, Some(&Err(err())), &Ok(()));
        assert_eq!(c.skipped, 0);
        let e_str = c
            .settings_save_err
            .as_ref()
            .expect("settings_save_err must be Some");
        assert!(!e_str.is_empty(), "captured error string must not be empty");
        assert!(
            e_str.contains('x'),
            "error string must embed the 'x' from err()"
        );
        assert!(c.library_save_err.is_none());
    }

    #[test]
    fn library_failure_only_emits_library_notice() {
        let c = notices_content(0, SkippedDetail::None, Some(&Ok(())), &Err(err()));
        assert_eq!(c.skipped, 0);
        assert!(c.settings_save_err.is_none());
        let e_str = c
            .library_save_err
            .as_ref()
            .expect("library_save_err must be Some");
        assert!(!e_str.is_empty(), "captured error string must not be empty");
        assert!(
            e_str.contains('x'),
            "error string must embed the 'x' from err()"
        );
    }

    #[test]
    fn all_three_failures_captured() {
        let c = notices_content(2, SkippedDetail::Archive, Some(&Err(err())), &Err(err()));
        assert_eq!(c.skipped, 2);
        assert_eq!(c.skipped_detail, SkippedDetail::Archive);
        assert!(c.settings_save_err.is_some());
        assert!(c.library_save_err.is_some());
    }

    #[test]
    fn settings_none_with_library_failure() {
        let c = notices_content(0, SkippedDetail::None, None, &Err(err()));
        assert!(c.settings_save_err.is_none());
        assert!(c.library_save_err.is_some());
    }

    #[test]
    fn skipped_and_library_failure_without_settings_tracking() {
        let c = notices_content(1, SkippedDetail::Archive, None, &Err(err()));
        assert_eq!(c.skipped, 1);
        assert!(c.settings_save_err.is_none());
        assert!(c.library_save_err.is_some());
    }

    #[test]
    fn skipped_and_settings_failure_captured() {
        let c = notices_content(1, SkippedDetail::None, Some(&Err(err())), &Ok(()));
        assert_eq!(c.skipped, 1);
        assert!(c.settings_save_err.is_some());
        assert!(c.library_save_err.is_none());
    }

    // ---- remove_books_with_rollback ---------------------------------------

    /// Build a library with three books, the middle one carrying a non-default
    /// reading position so the `add()`-trap (which resets last_page to 0) would
    /// surface in a byte-comparison rollback test.
    fn lib_with_three() -> Library {
        let mut lib = Library::new();
        for name in ["a.cbz", "b.cbz", "c.cbz"] {
            assert!(lib.add(PathBuf::from(format!("/manga/{name}"))).is_some());
        }
        assert!(lib.set_last_page(Path::new("/manga/b.cbz"), 17));
        assert!(lib.set_page_count(Path::new("/manga/b.cbz"), NonZeroUsize::new(80).unwrap()));
        lib
    }

    #[test]
    fn rollback_restores_library_byte_identically_on_save_failure() {
        // The whole point of the rollback path: a failed save must leave the
        // in-memory shelf byte-for-byte identical to its pre-removal state, even
        // when removed books carried non-default last_page/page_count (the
        // add()-trap would otherwise reset those and the JSON would differ).
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
            lib.last_page(Path::new("/manga/b.cbz")),
            17,
            "restored book keeps its last_page (add() would reset it to 0)"
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
        // A stale selection path (matching no stored book) is reported as
        // not_found, NOT counted among removed — so the user-facing "removed N"
        // never over-counts a path that was already gone.
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

    // ---- confirm_delete_content -------------------------------------------
    //
    // These build a real En `FluentLanguageLoader` via `Localizer`, so they
    // exercise the actual `i18n::dynamic::confirm_delete_*` functions (cluster
    // A2). Assertions are STRUCTURAL (line counts, the title's count digit,
    // presence/absence of the optional lines) rather than byte-exact strings, so
    // they pin this builder's composition without coupling to A2's exact wording.

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
        // 12 books total; narrow the query to "book00" (exact-prefix, substring
        // match) so only book00 is visible, but select all 12.
        //   count(12) > CAP(10)           ⇒ "…and 2 more" line at index CAP.
        //   visible_selected(1) < count   ⇒ outside-search line at index CAP+1.
        // The two counts (M=2 vs N=11) are distinct, so the assertions are tight.
        // Ordering contract: the "and M more" line ALWAYS precedes the
        // outside-search line.
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
        // Projection drift: a selected path with no library entry is skipped from
        // the title LIST but still counts toward the title count (honesty rule).
        // The unresolvable path is also (necessarily) outside the visible search,
        // so an outside-search line is appended after the single resolvable title.
        let (lib, search, mut sel) = delete_fixture(1);
        sel.toggle(PathBuf::from("/manga/book00.cbz")); // resolvable
        sel.toggle(PathBuf::from("/manga/ghost.cbz")); // not in library
        let loc = en_loader();
        let content = confirm_delete_content(loc.loader(), &sel, &search, &lib, None);

        // The single resolvable title leads the body; the unresolvable path never
        // appears as its own title line.  The ghost is also outside the visible
        // search projection, so an outside-search line is always appended after
        // the single resolvable title: body_lines = [title, outside-search] → len 2.
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

    // ---- derive_title (empty-book fallback derivation) --------------------
    //
    // `derive_title` is the path-only fallback the empty-book reject path uses
    // when the source was never added (so no stored `Book::title` exists). It
    // must mirror `gashuu_core::Book::from_path`'s derivation exactly: directory
    // name for a folder, file stem for a file, lossy full path as a last resort.

    #[test]
    fn derive_title_uses_file_stem_for_a_file() {
        // A non-existent file path is treated as a file (is_dir() is false), so
        // the title is the stem with the extension dropped — matching Book.
        assert_eq!(
            derive_title(Path::new("/manga/Cool Title.cbz")),
            "Cool Title"
        );
    }

    #[test]
    fn derive_title_drops_only_the_last_extension() {
        // `file_stem` drops a single trailing extension, leaving any earlier dots
        // intact — the same behavior `Book` inherits from `Path::file_stem`.
        assert_eq!(derive_title(Path::new("/manga/Vol.1.cbz")), "Vol.1");
    }

    #[test]
    fn derive_title_uses_directory_name_for_a_folder() {
        // A real on-disk directory takes the is_dir() branch and yields its final
        // component (no extension stripping), matching Book's folder derivation.
        let root = tempfile::tempdir().expect("tempdir");
        let folder = root.path().join("My Series");
        std::fs::create_dir(&folder).expect("create folder");
        assert_eq!(derive_title(&folder), "My Series");
    }

    #[test]
    fn derive_title_falls_back_to_full_path_when_no_stem() {
        // A path with no file stem (e.g. the root) falls back to the lossy full
        // path string so the title is never empty — Book's final fallback.
        let title = derive_title(Path::new("/"));
        assert!(
            !title.is_empty(),
            "the derived title must never be empty, got {title:?}"
        );
    }

    // ---- OpenOutcome::EmptyBookRemoved (shape the formatter consumes) ------

    #[test]
    fn empty_book_removed_outcome_constructs_and_matches() {
        // Pin the variant shape `main.rs`'s finalize_open formats against: the
        // three named fields (title, removed, save_error) destructure as expected.
        let outcome = OpenOutcome::EmptyBookRemoved {
            title: "Empty Book".to_string(),
            removed: true,
            save_error: Some("disk full".to_string()),
        };
        match outcome {
            OpenOutcome::EmptyBookRemoved {
                title,
                removed,
                save_error,
            } => {
                assert_eq!(title, "Empty Book");
                assert!(removed);
                assert_eq!(save_error.as_deref(), Some("disk full"));
            }
            other => panic!("expected EmptyBookRemoved, got {other:?}"),
        }
    }

    #[test]
    fn empty_book_removed_outcome_carries_not_removed_no_error() {
        // The "never added" case: nothing removed, so no save was attempted and
        // save_error stays None.
        let outcome = OpenOutcome::EmptyBookRemoved {
            title: "Ghost".to_string(),
            removed: false,
            save_error: None,
        };
        match outcome {
            OpenOutcome::EmptyBookRemoved {
                title,
                removed,
                save_error,
            } => {
                assert_eq!(title, "Ghost");
                assert!(!removed);
                assert!(save_error.is_none());
            }
            other => panic!("expected EmptyBookRemoved, got {other:?}"),
        }
    }
}
