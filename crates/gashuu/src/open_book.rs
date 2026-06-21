//! The "open a book" application use case, extracted out of `main.rs`.
//!
//! [`OpenBookUseCase`] bundles the seven shared collaborators the open path
//! coordinates (state, settings, viewport, library, thumbs, covers, search) as
//! fields, so the open sites call [`OpenBookUseCase::run`] with just the per-call
//! `ui`, `path`, and `skipped_detail` instead of threading a nine-argument free
//! fn under `#[allow(clippy::too_many_arguments)]`. It touches Slint (status
//! text, carousel rebuild, thumbnail launch), so it lives in the UI crate.

use std::cell::RefCell;
use std::path::Path;
use std::rc::Rc;

use gashuu_core::{CoreError, Library, Settings, ThumbnailCache};
use slint::ComponentHandle;

use crate::carousel::{bind_carousel_model, build_carousel_model, cover_requests};
use crate::cover_loader::{purge_cover, CoverController};
use crate::library_model::LibrarySearchState;
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
        let policy = settings.borrow().archive_policy();
        let opened = state.borrow_mut().open_folder_with_policy(path, policy);
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
        // `page_count_opt()` returns a `Copy` `Option<NonZeroUsize>`; the `state.borrow()` drops at the
        // `;` so it cannot conflict with the `library.borrow_mut()` below.
        let page_count = state.borrow().page_count_opt();
        // Reject an empty book: the source opened cleanly but has zero pages, so it
        // must NOT be entered in the viewer. Bail out HERE, before `register_opened`,
        // so the bypassed side effects pinned by the spec never run — no recents
        // push, no settings save (deferred below), no `register_opened`, no per-book
        // view resolve, no library-save-on-open, no carousel rebuild, no thumbnail
        // start. The cover purge now happens HERE too, inside `remove_empty_book`
        // (deliberate Wave-2 #150 change). When `canonical` is None we cannot
        // identify the book, so we keep the existing behavior and fall through to the
        // warning branch below.
        //
        // Reviewer note: `ViewerState` still holds this empty source after the
        // bail-out; that is inert (page_count == 0, the viewer is never shown) and
        // the next successful open replaces it — by design, with no restore machinery.
        if page_count.is_none() {
            if let Some(c) = canonical.as_deref() {
                // The shared transaction: title capture → remove → save → cover
                // purge. Wave-2 #150 DELIBERATELY added the purge here — the old
                // "purge only in the cover-load path" asymmetry left a removed
                // book's cached cover as unreachable garbage.
                let removal = remove_empty_book(library, c);
                return OpenOutcome::EmptyBookRemoved {
                    title: removal.title,
                    removed: removal.removed,
                    save_error: removal.save_error,
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
        // the type END-TO-END: `page_count_opt()` produced the `Option<NonZeroUsize>`
        // at the read boundary, so there is no wrapping at this call site. A
        // zero-page open never reaches here (the empty-book bail-out above returned
        // for `None`), so `page_count` is always `Some(…)` in practice; the type
        // keeps the contract honest without a `> 0` guard / `debug_assert` here.
        // Borrow discipline: it holds `library.borrow_mut()` only for the
        // `let reg = ...` line (released at its `;`, before the `jump_to` below);
        // `state.borrow_mut().jump_to(...)` is a separate statement on a distinct
        // `RefCell`.
        let count_changed = if let Some(c) = canonical.as_deref() {
            let reg = library.borrow_mut().register_opened(c, page_count);
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

/// Result of [`remove_empty_book`]: the removed book's display title (captured
/// BEFORE removal), whether the book was actually present (and thus removed),
/// and the pre-captured (untranslated) library-save error — `None` when nothing
/// was removed (no save attempted) or the save succeeded.
pub(crate) struct EmptyBookRemoval {
    pub(crate) title: String,
    pub(crate) removed: bool,
    pub(crate) save_error: Option<String>,
}

/// The single home of the empty-book removal transaction: capture the display
/// title (stored `Book::title` preferred, `gashuu_core::display_title` fallback
/// when the book was never added) → `Library::remove` (idempotent) → save (only
/// when something was removed) → best-effort cover purge. Both the open-time
/// bail-out and the cover-load signal handler call this; callers compose
/// notices / rebuild the carousel from the returned data.
pub(crate) fn remove_empty_book(library: &RefCell<Library>, path: &Path) -> EmptyBookRemoval {
    // The `borrow_mut` is confined to this statement and drops at the `;`.
    let removal = remove_empty_book_with(&mut library.borrow_mut(), path, |l| l.save());
    // Best-effort purge, OUTSIDE the seam so the transaction logic stays
    // headless-testable. mtime drift / missing file is expected and only
    // warned inside `purge_cover`; cache-construction failure skips it.
    if removal.removed {
        match ThumbnailCache::new() {
            Ok(cache) => purge_cover(&cache, path),
            Err(e) => {
                tracing::warn!(error = %e, "cover cache unavailable; skipping cover purge on empty-book removal");
            }
        }
    }
    removal
}

/// Effect-seam twin of [`remove_empty_book`] (same shape as
/// `remove_books_with_rollback`): the save is injected so the transaction's
/// decisions — title preference, save-only-when-removed, error pre-capture —
/// are unit-testable without disk I/O or a cover cache. Unlike that twin it
/// deliberately does NOT roll back on a save failure: an empty book is
/// invalid-by-definition (ADR-0009), so the in-memory removal stands and only
/// the error is reported.
fn remove_empty_book_with(
    lib: &mut Library,
    path: &Path,
    save: impl FnOnce(&Library) -> Result<(), CoreError>,
) -> EmptyBookRemoval {
    // Prefer the stored Book's title; fall back to the core path-derivation
    // when the book was never added. Captured BEFORE removal.
    let title = lib
        .books()
        .iter()
        .find(|b| b.path() == path)
        .map(|b| b.title().to_string())
        .unwrap_or_else(|| gashuu_core::display_title(path));
    // `Library::remove` is idempotent and returns false when the book is
    // absent; it also clears `last_opened` when it pointed at this book.
    let removed = lib.remove(path);
    // Persist only when something was actually removed; otherwise the shelf
    // is unchanged so there is nothing to save.
    let save_error = if removed {
        match save(lib) {
            Ok(()) => None,
            Err(e) => {
                tracing::error!(error = %e, "failed to save library after removing empty book");
                Some(format!("{e}"))
            }
        }
    } else {
        None
    };
    EmptyBookRemoval {
        title,
        removed,
        save_error,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

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

    // ---- remove_empty_book_with (the empty-book removal transaction) -------

    #[test]
    fn remove_empty_book_removes_saves_and_reports_stored_title() {
        let mut lib = Library::new();
        let path = PathBuf::from("/manga/Empty Vol.cbz");
        lib.add(path.clone());
        let saves = std::cell::Cell::new(0);
        let removal = remove_empty_book_with(&mut lib, &path, |_| {
            saves.set(saves.get() + 1);
            Ok(())
        });
        assert!(removal.removed);
        assert_eq!(removal.title, "Empty Vol", "stored title preferred");
        assert_eq!(removal.save_error, None);
        assert_eq!(saves.get(), 1, "removal must persist exactly once");
        assert!(lib.books().is_empty());
    }

    #[test]
    fn remove_empty_book_absent_book_skips_save_and_derives_title() {
        let mut lib = Library::new();
        let path = PathBuf::from("/manga/Never Added.cbz");
        let removal = remove_empty_book_with(&mut lib, &path, |_| -> Result<(), CoreError> {
            panic!("save must not be attempted when nothing was removed")
        });
        assert!(!removal.removed);
        assert_eq!(removal.save_error, None);
        assert_eq!(
            removal.title,
            gashuu_core::display_title(&path),
            "fallback is the core derivation"
        );
    }

    #[test]
    fn remove_empty_book_captures_save_error_and_keeps_removal() {
        let mut lib = Library::new();
        let path = PathBuf::from("/manga/Empty.cbz");
        lib.add(path.clone());
        let removal = remove_empty_book_with(&mut lib, &path, |_| Err(err()));
        assert!(
            removal.removed,
            "the in-memory removal stands on save failure"
        );
        let detail = removal.save_error.expect("failure must be pre-captured");
        assert!(!detail.is_empty());
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
