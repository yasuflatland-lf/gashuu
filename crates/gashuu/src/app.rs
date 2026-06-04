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
use std::path::Path;
use std::rc::Rc;

use gashuu_core::{CoreError, Library, Settings};
use slint::ComponentHandle;

use crate::carousel::{bind_carousel_model, build_carousel_model, cover_requests};
use crate::cover_loader::CoverController;
use crate::library_model::LibrarySearchState;
use crate::thumbnail_strip::ThumbnailController;
use crate::viewer_state::ViewerState;
use crate::viewport::ViewportState;
use crate::{write_back_position, write_back_view_override, ViewerWindow};

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
        // book is open (open_file() is None).
        write_back_view_override(state, viewport, library);
        // Bind the result first so the `state.borrow_mut()` temporary drops before the
        // `Ok` arm reads `state` again (a borrow held across the match would
        // double-borrow-panic at the `canonical = state.borrow().open_file()...` read
        // below).
        let opened = state.borrow_mut().open_folder(path);
        // The settings-save outcome, captured so it can be surfaced AFTER refresh
        // (composing onto the status line). `None` when no save was attempted
        // (recent-files tracking off); `Some(result)` otherwise. Surfacing before
        // refresh would be clobbered by the spread/status push.
        let settings_save: Option<Result<(), CoreError>> = match opened {
            Ok(()) => {
                tracing::info!(path = %path.display(), "opened source");
                let mut s = settings.borrow_mut();
                if s.track_recent_files {
                    // Persist the recents update (and the global Settings as-is) on
                    // open. We intentionally do NOT reconcile the runtime view modes
                    // into Settings here — the runtime currently holds the
                    // just-opened/outgoing book's per-book modes, not the global
                    // defaults; global view modes change only via the Library settings
                    // dialog and the no-book-open exit path. This save writes the
                    // recents list + cache/preload/track plus the UNCHANGED global
                    // view-mode fields.
                    s.push_recent(path.to_path_buf());
                    let result = s.save();
                    if let Err(e) = &result {
                        tracing::error!(error = %e, "failed to save settings on open");
                    }
                    Some(result)
                } else {
                    None
                }
            }
            Err(e) => {
                tracing::error!(error = %e, "failed to open source");
                return OpenOutcome::Error(format!("{e}"));
            }
        };
        // The CANONICAL key the source was opened under (read from `open_file`),
        // not the raw dialog `path`, which may be a non-canonical dialog path. It is
        // the same key `last_page`/`set_page_count`/`write_back_position` use. The
        // `state.borrow()` `Ref` drops at the `;`.
        let canonical = state.borrow().open_file().map(Path::to_path_buf);
        // `page_count()` returns a `Copy` `usize`; the `state.borrow()` drops at the
        // `;` so it cannot conflict with the `library.borrow_mut()` below.
        let page_count = state.borrow().page_count();
        // `register_opened` performs the idempotent add, the page-count back-fill,
        // and the resume lookup as one domain rule. The unknown total is now carried
        // by the type: `NonZeroUsize::new(page_count)` maps a zero-page open (the
        // unknown sentinel an empty folder or a fully skipped archive opens with) to
        // `None`, so `register_opened` skips the back-fill for it — no more `> 0`
        // guard / `debug_assert` at this call site. Borrow discipline: it holds
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
        assert!(c.settings_save_err.is_some());
        assert!(c.library_save_err.is_none());
    }

    #[test]
    fn library_failure_only_emits_library_notice() {
        let c = notices_content(0, SkippedDetail::None, Some(&Ok(())), &Err(err()));
        assert_eq!(c.skipped, 0);
        assert!(c.settings_save_err.is_none());
        assert!(c.library_save_err.is_some());
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
}
