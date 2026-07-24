//! The "open a book" application use case, extracted out of `main.rs`.
//!
//! [`OpenBookUseCase`] bundles the four headless collaborators the open path
//! coordinates (state, settings, viewport, library) as fields, so the open sites
//! call [`OpenBookUseCase::run`] with just the per-call `path` (`skipped_detail`
//! is derived internally). `run` is fully headless — it returns an
//! [`OpenOutcome`] and `main.rs`'s `finalize_open` applies every UI effect
//! (status text, viewer refresh, carousel rebuild, thumbnail launch), the same
//! headless-use-case + UI-finalize split as `RemoveBooksUseCase` / `finalize_remove`.

use std::cell::RefCell;
use std::path::Path;
use std::rc::Rc;

use gashuu_core::{CoreError, Library, Settings, ThumbnailCache};

use crate::cover_loader::purge_cover;
use crate::viewer_state::ViewerState;
use crate::viewport::ViewportState;
use crate::{route_view_modes_to_sink, write_back_position, ViewModeRoute};

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
    /// The open succeeded. `main.rs`'s `finalize_open` refreshes the viewer,
    /// launches thumbnails, formats `notices` via `i18n::dynamic::format_notices`,
    /// and — when `count_changed` (the open back-filled a page count) — rebuilds
    /// the library carousel so the shelf reflects the new count.
    Success {
        notices: NoticesContent,
        count_changed: bool,
    },
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
    EmptyBookRejected {
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
    /// The open source was an archive; skip reasons name zip-slip / oversized entries.
    Archive,
}

/// Coordinates the "open a book" use case. The four headless collaborators it
/// threads are fields, so the open sites call [`OpenBookUseCase::run`] with just
/// a `path`; `main.rs`'s `finalize_open` applies the UI effects (symmetry with
/// `RemoveBooksUseCase`).
pub(crate) struct OpenBookUseCase {
    state: Rc<RefCell<ViewerState>>,
    settings: Rc<RefCell<Settings>>,
    viewport: Rc<RefCell<ViewportState>>,
    library: Rc<RefCell<Library>>,
}

impl OpenBookUseCase {
    // Four explicit collaborators (#151 explicit-handle policy: named params, not an AppState bundle).
    pub(crate) fn new(
        state: Rc<RefCell<ViewerState>>,
        settings: Rc<RefCell<Settings>>,
        viewport: Rc<RefCell<ViewportState>>,
        library: Rc<RefCell<Library>>,
    ) -> Self {
        Self {
            state,
            settings,
            viewport,
            library,
        }
    }

    /// Open `path` and mutate the shared state: write back the previous position,
    /// open the source, reconcile + save settings (when recent-files tracking is
    /// on), register + save the library, and resolve the per-book view modes.
    ///
    /// Returns [`OpenOutcome::Error`] with a pre-captured error string on failure,
    /// or [`OpenOutcome::Success`] with neutral [`NoticesContent`] and the
    /// `count_changed` flag on success. `main.rs`'s `finalize_open` applies every
    /// UI effect (viewer refresh, carousel rebuild, thumbnail launch, status notices).
    ///
    /// The `skipped_detail` suffix is DERIVED internally from `path`
    /// ([`SkippedDetail::None`] for folders, [`SkippedDetail::Archive`] for
    /// archives), not passed by the caller.
    pub(crate) fn run(&self, path: &Path) -> OpenOutcome {
        // Alias the fields so the body reads identically to its pre-extraction form.
        let state = &self.state;
        let settings = &self.settings;
        let viewport = &self.viewport;
        let library = &self.library;

        // Write back the current book's position before we replace the source.
        // `open_file()` is None when no book is open, so this is a no-op then.
        write_back_position(state, library);
        // Capture the OUTGOING book's view modes before the source is replaced, so a
        // bare D/R/C/fit toggle persists without the settings dialog (ADR-0007 clobber-trap).
        route_view_modes_to_sink(
            ViewModeRoute::OpenDifferentBook,
            state,
            viewport,
            settings,
            library,
        );
        // Bind the result first so the `borrow_mut()` temporary drops before the match;
        // a borrow held across the match would double-borrow-panic at the read below.
        let policy = settings.borrow().archive_policy();
        let opened = state.borrow_mut().open_path_with_policy(path, policy);
        // Discriminate the open result only — recents push + settings save are DEFERRED
        // past the empty-book check so a zero-page source bypasses them (spec-pinned).
        match opened {
            Ok(()) => {
                tracing::info!(path = %path.display(), "opened source");
            }
            Err(e) => {
                tracing::error!(error = %e, "failed to open source");
                return OpenOutcome::Error(format!("{e}"));
            }
        }
        // The CANONICAL key the source was opened under (from `open_file`), not the raw
        // dialog `path` — the same key `resume_page`/`set_page_count`/write-back use.
        let canonical = state.borrow().open_file().map(Path::to_path_buf);
        // `page_count_opt()` returns a `Copy` `Option<NonZeroUsize>`; the `state.borrow()` drops at the
        // `;` so it cannot conflict with the `library.borrow_mut()` below.
        let page_count = state.borrow().page_count_opt();
        // Reject an empty book: bail HERE, before `register_opened`, so spec-pinned side
        // effects bypass (cover purge in `remove_empty_book`, #150); source left inert by design.
        if page_count.is_none() {
            if let Some(c) = canonical.as_deref() {
                // Shared transaction: title capture → remove → save → cover purge. #150
                // added the purge here; the old cover-load-only asymmetry orphaned cached covers.
                let removal = remove_empty_book(library, c);
                return OpenOutcome::EmptyBookRejected {
                    title: removal.title,
                    removed: removal.removed,
                    save_error: removal.save_error,
                };
            }
        }
        // Non-empty path: run the DEFERRED recents push + settings save now (bypassed for
        // zero-page). Does NOT reconcile runtime view modes into Settings (per-book, not global).
        let settings_save: Option<Result<(), CoreError>> = {
            let mut s = settings.borrow_mut();
            if s.track_recent_sources {
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
        // `register_opened` = idempotent add + back-fill + resume lookup; its `borrow_mut`
        // is confined to the `let reg` line and released before `jump_to`, avoiding a clash.
        let count_changed = if let Some(c) = canonical.as_deref() {
            let reg = library.borrow_mut().register_opened(c, page_count);
            // Resume at the recorded position; for a never-read book `last_viewed` is 0
            // and `jump_to(0)` is a no-op when the index is already 0.
            state.borrow_mut().jump_to(reg.resume.last_viewed());
            reg.count_changed
        } else {
            // Unreachable in practice: a successful open always sets `open_file`. Log if
            // the invariant breaks so the book-not-registered failure isn't silent.
            tracing::warn!(
                path = %path.display(),
                "open_file was None after a successful open; book not registered in library"
            );
            false
        };
        // Resolve this book's per-book override (empty => globals) and apply it BEFORE the
        // first refresh, after `jump_to`, so the resumed page re-anchors to a valid leading.
        let resolved = {
            let overrides = canonical
                .as_deref()
                .map(|c| library.borrow().overrides_for(c))
                .unwrap_or_default();
            overrides.resolve(&settings.borrow())
        };
        state
            .borrow_mut()
            .apply_resolved_view(resolved, &mut viewport.borrow_mut());
        // Persist the registered book + back-filled page count; this MUST stay a SYNCHRONOUS
        // save, else a detached write could land after a later save and revert position/drop book.
        if let Err(e) = library.borrow().save() {
            tracing::error!(error = %e, "failed to save library on open");
        }
        let skipped = state.borrow().last_open_skipped();
        let skipped_detail = if path.is_dir() {
            SkippedDetail::None
        } else {
            SkippedDetail::Archive
        };
        // The on-open library save is best-effort (logged, re-saved at the next leave
        // point), so pass `Ok` for the library-save slot; the settings save is surfaced.
        // On a `count_changed` back-fill the carousel rebuild + thumbnail launch are
        // applied by `finalize_open` (success path only), keeping this use case headless.
        OpenOutcome::Success {
            notices: notices_content(skipped, skipped_detail, settings_save.as_ref(), &Ok(())),
            count_changed,
        }
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
pub(crate) struct EmptyBookOutcome {
    pub(crate) title: String,
    pub(crate) removed: bool,
    pub(crate) save_error: Option<String>,
}

/// The display title to show for `path`: the stored `Book::title` when the book
/// is in the library, else the core path-derivation (`gashuu_core::display_title`).
/// Single-homes the "stored title preferred, path-derived fallback" rule shared
/// by the empty-book removal transaction and the failed-open status message.
pub(crate) fn book_display_title(lib: &Library, path: &Path) -> String {
    lib.books()
        .iter()
        .find(|b| b.path() == path)
        .map(|b| b.title().to_string())
        .unwrap_or_else(|| gashuu_core::display_title(path))
}

/// The single home of the empty-book removal transaction: capture the display
/// title (stored `Book::title` preferred, `gashuu_core::display_title` fallback
/// when the book was never added) → `Library::remove` (idempotent) → save (only
/// when something was removed) → best-effort cover purge. Both the open-time
/// bail-out and the cover-load signal handler call this; callers compose
/// notices / rebuild the carousel from the returned data.
pub(crate) fn remove_empty_book(library: &RefCell<Library>, path: &Path) -> EmptyBookOutcome {
    // The `borrow_mut` is confined to this statement and drops at the `;`.
    let removal = remove_empty_book_with(&mut library.borrow_mut(), path, |l| l.save());
    // Best-effort purge OUTSIDE the seam so the transaction stays headless-testable.
    // mtime drift / missing file is expected (only warned); cache-construction failure skips it.
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
) -> EmptyBookOutcome {
    // Prefer the stored Book's title; fall back to the core path-derivation
    // when the book was never added. Captured BEFORE removal.
    let title = book_display_title(lib, path);
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
    EmptyBookOutcome {
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

    // ---- synchronous open-time library save (issue #360) ------------------

    #[test]
    fn synchronous_open_save_is_visible_on_disk_before_returning() {
        // Regression guard for #360: the open-time save was a detached `thread::spawn`,
        // so a slow write could land after a later save and revert position/drop a book.
        let dir = tempfile::tempdir().expect("tempdir");
        let store = dir.path().join("library.json");

        let mut lib = Library::new();
        let book = PathBuf::from("/manga/Just Opened Vol.cbz");
        // Mirror the open path: register with a known page count, then save synchronously
        // (production `library.borrow().save()` is `save_to(data_path())`).
        lib.register_opened(&book, std::num::NonZeroUsize::new(42));
        lib.save_to(&store).expect("synchronous save must succeed");

        // The write completed before control returned here (no background thread
        // to await): reloading immediately reflects the just-opened book.
        let reloaded = Library::load_from(&store).expect("reload must succeed");
        let stored = reloaded
            .books()
            .iter()
            .find(|b| b.path() == book)
            .expect("the just-opened book must be persisted synchronously");
        assert_eq!(
            stored.page_count_opt(),
            Some(42),
            "the back-filled page count must be on disk after the synchronous save"
        );
        assert_eq!(
            reloaded.last_opened(),
            Some(book.as_path()),
            "last_opened must point at the just-opened book on disk"
        );
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

    // ---- OpenOutcome::EmptyBookRejected (shape the formatter consumes) ------

    #[test]
    fn empty_book_rejected_outcome_constructs_and_matches() {
        // Pin the variant shape `main.rs`'s finalize_open formats against: the
        // three named fields (title, removed, save_error) destructure as expected.
        let outcome = OpenOutcome::EmptyBookRejected {
            title: "Empty Book".to_string(),
            removed: true,
            save_error: Some("disk full".to_string()),
        };
        match outcome {
            OpenOutcome::EmptyBookRejected {
                title,
                removed,
                save_error,
            } => {
                assert_eq!(title, "Empty Book");
                assert!(removed);
                assert_eq!(save_error.as_deref(), Some("disk full"));
            }
            other => panic!("expected EmptyBookRejected, got {other:?}"),
        }
    }

    #[test]
    fn empty_book_rejected_outcome_carries_not_removed_no_error() {
        // The "never added" case: nothing removed, so no save was attempted and
        // save_error stays None.
        let outcome = OpenOutcome::EmptyBookRejected {
            title: "Ghost".to_string(),
            removed: false,
            save_error: None,
        };
        match outcome {
            OpenOutcome::EmptyBookRejected {
                title,
                removed,
                save_error,
            } => {
                assert_eq!(title, "Ghost");
                assert!(!removed);
                assert!(save_error.is_none());
            }
            other => panic!("expected EmptyBookRejected, got {other:?}"),
        }
    }

    // ---- OpenBookUseCase::run headless branches (unlocked by the headless refactor) ----
    //
    // These drive `run` with no UI window — the win of headless-ization. Only the
    // branches that reach no real `save()` are covered: `run`'s success path calls the real
    // `library.borrow().save()` (a `data_path()` write), so it is deliberately NOT driven
    // end-to-end here (it would clobber the developer's real `library.json`, and the codebase
    // convention is to never call the real `save()` in tests). The synchronous open-time save
    // invariant (#360) stays covered hermetically above via `save_to`.
    //
    // Keep these fixtures NEVER-ADDED: opening an empty source for a book already in the
    // library would remove it and reach the real `library.save()` — clobbering real data.

    /// Build a use case over fresh in-memory collaborators; `library` is passed in so
    /// the test can inspect it after `run`. `Settings::default()` has
    /// `track_recent_sources = false`, so no settings save is attempted either.
    fn use_case_over(library: Rc<RefCell<Library>>) -> OpenBookUseCase {
        let settings = Settings::default();
        let state = Rc::new(RefCell::new(ViewerState::from_settings(&settings)));
        let viewport = Rc::new(RefCell::new(ViewportState::from_settings(&settings)));
        OpenBookUseCase::new(state, Rc::new(RefCell::new(settings)), viewport, library)
    }

    #[test]
    fn run_returns_error_for_unopenable_path() {
        // A non-existent path can never be opened; `run` bails at the open error BEFORE any
        // register/save, so the failure path is fully hermetic (no disk I/O) and registers nothing.
        let dir = tempfile::tempdir().expect("tempdir");
        let missing = dir.path().join("does-not-exist");
        let library = Rc::new(RefCell::new(Library::new()));
        let use_case = use_case_over(Rc::clone(&library));

        let outcome = use_case.run(&missing);

        assert!(
            matches!(outcome, OpenOutcome::Error(_)),
            "an unopenable path must yield Error, got {outcome:?}"
        );
        assert!(
            library.borrow().books().is_empty(),
            "a failed open registers nothing"
        );
    }

    #[test]
    fn run_rejects_empty_source_and_does_not_enter_viewer() {
        // An empty folder opens cleanly with zero pages: `run` returns `EmptyBookRejected`.
        // The book was never in the library, so the removal is a no-op and NO save is
        // attempted — fully hermetic (no disk I/O).
        let dir = tempfile::tempdir().expect("tempdir");
        let library = Rc::new(RefCell::new(Library::new()));
        let use_case = use_case_over(Rc::clone(&library));

        let outcome = use_case.run(dir.path());

        match outcome {
            OpenOutcome::EmptyBookRejected {
                removed,
                save_error,
                ..
            } => {
                assert!(!removed, "a never-added empty source is not removed");
                assert!(
                    save_error.is_none(),
                    "no save is attempted when nothing was removed"
                );
            }
            other => panic!("expected EmptyBookRejected, got {other:?}"),
        }
        assert!(
            library.borrow().books().is_empty(),
            "an empty source is never registered in the library"
        );
    }
}
