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
use crate::{reconcile_settings, refresh, write_back_position, ViewerWindow};

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
    /// register + save the library, refresh, compose status notices, rebuild
    /// the carousel, and launch thumbnails.
    ///
    /// `skipped_detail` is `""` for folders and `" (zip-slip or oversized)"`
    /// for archives. Behaviour-preserving move of the former `open_and_present`.
    pub(crate) fn run(&self, ui: &ViewerWindow, path: &Path, skipped_detail: &str) {
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
        // Bind the result first so the `state.borrow_mut()` temporary drops before the
        // `Ok` arm reads `state` again (a borrow held across the match would
        // double-borrow-panic at the `reconcile_settings(&state.borrow(), ..)` below).
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
                    // Reconcile runtime display modes into Settings just before this
                    // open-time save (the only save on this path), reusing the mutable
                    // borrow `s`. `state`/`viewport` are distinct RefCells, so their
                    // immutable borrows can't conflict with `s` (and the `borrow_mut()`
                    // from `open_folder` above already dropped via the `opened` binding).
                    reconcile_settings(&state.borrow(), &viewport.borrow(), &mut s);
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
                ui.set_status_text(format!("Error: {e}").into());
                return;
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
        refresh(ui, &state.borrow(), viewport);
        // Compose extra notices onto the status line WITHOUT replacing it (the
        // refresh above set the base spread status). Each notice chains onto the
        // current text as `{base} — {detail}` (mirrors the skipped-entries
        // pattern). WHICH notices appear — order: skipped, then settings-save
        // failure, then library-save failure — is the pure `status_notices`
        // decision below, unit-tested without a UI (mirrors `position_to_write_back`).
        let skipped = state.borrow().last_open_skipped();
        for detail in status_notices(
            skipped,
            skipped_detail,
            settings_save.as_ref(),
            &library_save,
        ) {
            let base = ui.get_status_text().to_string();
            ui.set_status_text(format!("{base} \u{2014} {detail}").into());
        }
        // When the stored page count was just back-filled/updated, rebuild the
        // carousel model so the home screen reflects the real total/progress when
        // the user returns to the library. Preserve the ACTIVE filter: recompute
        // the shared search state and rebuild from its visible indices (not the
        // full library), so an open-time backfill never resurrects filtered-out
        // books. The carousel's focused index is a separate Slint property and is
        // intentionally NOT reset here — this is a page-count refresh, not a filter
        // update. Each `library.borrow()` is confined to one statement and dropped
        // before the next (in particular before `covers.start`'s `borrow_mut`).
        if count_changed {
            let indices = {
                let lib = library.borrow();
                let mut search = self.search.borrow_mut();
                search.recompute(&lib);
                search.visible_indices().to_vec()
            };

            let model = {
                let lib = library.borrow();
                build_carousel_model(&lib, &indices)
            };
            bind_carousel_model(ui, model);

            // The rebuild reset each visible row's cover to a placeholder; re-stream
            // the covers (hits paint now, misses regenerate). The epoch bump
            // supersedes any covers still streaming from the pre-rebuild model.
            let cover_reqs = {
                let lib = library.borrow();
                cover_requests(&lib, &indices)
            };
            covers.start(ui.as_weak(), library, cover_reqs);
        }
        // Kick off parallel thumbnail generation for the newly opened source.
        thumbs.start(
            ui.as_weak(),
            state.borrow().current_source(),
            state.borrow().page_count(),
        );
    }
}

/// The extra status notices appended after `refresh` set the base spread
/// status, in display order: skipped entries, then a settings-save failure,
/// then a library-save failure. Pure so the compose order is unit-tested
/// without a UI. `None` `settings_save` means no save was attempted
/// (recent-files tracking off) and therefore adds no notice.
pub(crate) fn status_notices(
    skipped: usize,
    skipped_detail: &str,
    settings_save: Option<&Result<(), CoreError>>,
    library_save: &Result<(), CoreError>,
) -> Vec<String> {
    let mut notices = Vec::new();
    if skipped > 0 {
        notices.push(format!("{skipped} entries skipped{skipped_detail}"));
    }
    if let Some(Err(e)) = settings_save {
        notices.push(format!("Failed to save settings: {e}"));
    }
    if let Err(e) = library_save {
        notices.push(format!("Failed to save library: {e}"));
    }
    notices
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
        // skipped=0, no settings save attempted, library saved fine -> nothing.
        assert!(status_notices(0, "", None, &Ok(())).is_empty());
        // skipped=0 even with a detail string present -> still nothing (the
        // detail only matters once `skipped > 0`).
        assert!(status_notices(0, " (zip-slip or oversized)", None, &Ok(())).is_empty());
    }

    #[test]
    fn skipped_only_emits_skipped_notice() {
        let notices = status_notices(3, " (zip-slip or oversized)", Some(&Ok(())), &Ok(()));
        assert_eq!(notices, vec!["3 entries skipped (zip-slip or oversized)"]);
    }

    #[test]
    fn settings_failure_only_emits_settings_notice() {
        let settings_err = Err(err());
        let notices = status_notices(0, "", Some(&settings_err), &Ok(()));
        assert_eq!(notices.len(), 1);
        assert!(notices[0].starts_with("Failed to save settings: "));
    }

    #[test]
    fn library_failure_only_emits_library_notice() {
        let library_err = Err(err());
        let notices = status_notices(0, "", Some(&Ok(())), &library_err);
        assert_eq!(notices.len(), 1);
        assert!(notices[0].starts_with("Failed to save library: "));
    }

    #[test]
    fn all_three_failures_emit_in_skipped_settings_library_order() {
        let settings_err = Err(err());
        let library_err = Err(err());
        let notices = status_notices(
            2,
            " (zip-slip or oversized)",
            Some(&settings_err),
            &library_err,
        );
        assert_eq!(notices.len(), 3);
        assert!(notices[0].starts_with("2 entries skipped"));
        assert!(notices[1].starts_with("Failed to save settings: "));
        assert!(notices[2].starts_with("Failed to save library: "));
    }

    #[test]
    fn settings_none_with_library_failure_emits_only_library_notice() {
        // Tracking-off (`None` settings_save) adds no settings notice even when
        // the library save fails.
        let library_err = Err(err());
        let notices = status_notices(0, "", None, &library_err);
        assert_eq!(notices.len(), 1);
        assert!(notices[0].starts_with("Failed to save library: "));
    }

    #[test]
    fn skipped_and_library_failure_without_settings_tracking_preserves_order() {
        let library_err = Err(err());
        let notices = status_notices(1, " (zip-slip or oversized)", None, &library_err);
        assert_eq!(notices.len(), 2);
        assert!(notices[0].starts_with("1 entries skipped (zip-slip or oversized)"));
        assert!(notices[1].starts_with("Failed to save library: "));
    }

    #[test]
    fn skipped_and_settings_failure_preserves_order() {
        let settings_err = Err(err());
        let notices = status_notices(1, "", Some(&settings_err), &Ok(()));
        assert_eq!(notices.len(), 2);
        assert!(notices[0].starts_with("1 entries skipped"));
        assert!(notices[1].starts_with("Failed to save settings: "));
    }
}
