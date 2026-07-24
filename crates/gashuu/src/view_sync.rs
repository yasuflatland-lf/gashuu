use crate::{viewer_state::ViewerState, viewport::ViewportState};
use gashuu_core::{FitMode, Library, ReadingDirection, ResolvedView, Settings, ViewOverride};
use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;

/// The leave/close point at which runtime view modes are persisted, naming WHERE
/// the runtime came from so [`route_view_modes_to_sink`] can route to the right sink.
/// One variant per production call site of the old write helpers.
pub(crate) enum ViewModeRoute {
    /// Settings dialog closed on the Library screen (screen 0): the dialog edits
    /// the GLOBAL defaults, so the runtime is reconciled into `Settings`.
    DialogClosedOnLibrary,
    /// Settings dialog closed on the Viewer screen (screen 1): the dialog edits
    /// the CURRENT book's per-book override.
    DialogClosedOnViewer,
    /// Leaving the viewer for the Library (↑): persist the open book's override.
    LeaveViewer,
    /// Opening a different book while one is open (`OpenBookUseCase::run`):
    /// persist the OUTGOING book's override before the source is replaced.
    OpenDifferentBook,
    /// App exit: persist the open book's override, then reconcile into the GLOBAL
    /// defaults ONLY when no book is open.
    AppExit,
}

/// THE single chokepoint that routes runtime view modes (direction/spread/cover/
/// fit) to their persistence sink. It is the ONLY caller of `apply_runtime_view_to_settings`
/// (runtime → GLOBAL `Settings`) and the only view_sync caller of
/// `write_back_view_override` (runtime → PER-BOOK override).
///
/// ADR-0007 clobber-trap, made structural here (it once shipped as a real bug):
/// once view modes became per-book with a global fallback, EVERY "copy runtime →
/// global" op (`apply_runtime_view_to_settings`) became a potential CLOBBER — the runtime may
/// hold a per-book value, so reconciling it would overwrite the GLOBAL default
/// with one book's preference. The routing match below is the invariant: the
/// GLOBAL sink is written ONLY by (a) the Library-screen settings dialog close and
/// (b) the no-book-open exit path; the PER-BOOK sink is written ONLY at leave
/// points (the Viewer-screen settings dialog close, the ↑ leave-viewer key, and
/// opening a different book while one is open). Note (a): the Library dialog
/// legitimately reconciles into global even while a book is loaded in
/// `ViewerState`, because the runtime was global-seeded
/// by `apply_global_view_to_runtime` at dialog open — so this path must NOT be
/// blanket-guarded on `open_file().is_none()`, or Library-dialog edits would be
/// dropped. The exit path keeps the per-book write FIRST, then the open-state
/// guard on the global reconcile.
pub(crate) fn route_view_modes_to_sink(
    route: ViewModeRoute,
    state: &Rc<RefCell<ViewerState>>,
    viewport: &Rc<RefCell<ViewportState>>,
    settings: &Rc<RefCell<Settings>>,
    library: &Rc<RefCell<Library>>,
) {
    match route {
        ViewModeRoute::DialogClosedOnLibrary => {
            apply_runtime_view_to_settings(
                &state.borrow(),
                &viewport.borrow(),
                &mut settings.borrow_mut(),
            );
        }
        ViewModeRoute::DialogClosedOnViewer
        | ViewModeRoute::LeaveViewer
        | ViewModeRoute::OpenDifferentBook => {
            write_back_view_override(state, viewport, library);
        }
        ViewModeRoute::AppExit => {
            // Per-book override FIRST (no-op if no book is open), so the open
            // book's modes are saved before the open-state-guarded global reconcile.
            write_back_view_override(state, viewport, library);
            if state.borrow().open_file().is_none() {
                apply_runtime_view_to_settings(
                    &state.borrow(),
                    &viewport.borrow(),
                    &mut settings.borrow_mut(),
                );
            }
        }
    }
}

/// Copy the runtime-owned display settings into the persisted `Settings` just
/// before saving. This is the SINGLE place `reading_direction`, `spread_mode`,
/// `cover_mode`, and `fit_mode` are written back to `Settings`, so a new
/// mode-mutation site can never "forget to mirror" — it only changes runtime
/// state, and the next save reconciles automatically. Reached only via
/// [`route_view_modes_to_sink`] (the routing chokepoint).
fn apply_runtime_view_to_settings(
    state: &ViewerState,
    viewport: &ViewportState,
    settings: &mut Settings,
) {
    settings.reading_direction = state.reading_direction();
    settings.spread_mode = state.spread_mode();
    settings.cover_mode = state.cover_mode();
    settings.fit_mode = viewport.fit_mode();
}

/// Snapshot the current runtime view modes as a `ResolvedView`.
///
/// Reads the three `ViewerState`-owned modes (direction/spread/cover) plus the
/// `ViewportState`-owned fit mode. Used by the Library-screen settings dialog
/// (issue #414): the dialog seeds the SHARED runtime with global defaults on
/// open (`apply_global_view_to_runtime`), which would clobber a still-open
/// book's runtime; snapshotting it here lets `on_close_settings` restore the
/// book's own runtime, so the later leave/exit write-back pins the BOOK's value
/// rather than the transiently-global one.
///
/// Borrow discipline: `state` and `viewport` are distinct `RefCell`s, so the
/// two shared borrows never conflict; both drop on return.
pub(crate) fn current_runtime_view(
    state: &Rc<RefCell<ViewerState>>,
    viewport: &Rc<RefCell<ViewportState>>,
) -> ResolvedView {
    let s = state.borrow();
    ResolvedView {
        reading_direction: s.reading_direction(),
        spread_mode: s.spread_mode(),
        cover_mode: s.cover_mode(),
        fit_mode: viewport.borrow().fit_mode(),
    }
}

/// Mirror the GLOBAL `Settings` view modes into the runtime (`ViewerState` for
/// direction/spread/cover, `ViewportState` for fit) — the inverse of
/// `apply_runtime_view_to_settings`. Used when the dialog edits the global defaults
/// (opening Library settings) and when resetting an open book to global.
/// This starts from `Settings`, not a `ResolvedView`, so the individual setters
/// remain intentional rather than routing through `apply_resolved_view`.
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
/// `open_file` is a real filesystem path; the folder/archive discrimination
/// happens inside `gashuu_core::display_title`, which checks `is_dir()` live
/// on the same real path. Borrow discipline: the single `state.borrow()` `Ref`
/// is confined to this function and drops on return.
pub(crate) fn current_book_name(state: &Rc<RefCell<ViewerState>>) -> String {
    let s = state.borrow();
    match s.open_file() {
        Some(path) => gashuu_core::display_title(path),
        None => String::new(),
    }
}

/// Pure helper: decide if and what to write back to the Library.
///
/// Returns `Some((canonical_path, page_index))` when a write-back should be
/// performed (a book is open), `None` otherwise. Extracted for table-testing
/// so the predicate can be verified independently of the effectful
/// `write_back_position` that actually calls `library.set_resume_page`.
fn position_to_write_back(open_file: Option<&Path>, page: usize) -> Option<(PathBuf, usize)> {
    open_file.map(|p| (p.to_path_buf(), page))
}

/// Write the current reading position back to the Library and persist.
///
/// Called at every leave point: ↑ to Library, opening a different book,
/// and app exit. `set_resume_page` returns `false` when the path is absent or
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
    // `set_resume_page` returns false when absent or unchanged; we persist
    // unconditionally for simplicity (short JSON write, idempotent on disk).
    library.borrow_mut().set_resume_page(&path, page);
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
///
/// `inherit_pending` is the "Reset to global" guard: when the open book was just
/// reset and no mode has changed since, the write-back must keep the override
/// EMPTY (`ViewOverride::none()`) rather than re-pin the runtime — otherwise
/// closing the dialog would instantly undo the reset. Cleared by any real mode
/// change (see `ViewerState`), so a re-selection after reset still pins normally.
///
/// Extracted (mirrors `position_to_write_back`) so the predicate is unit-tested
/// without the effectful `set_overrides` + `save`.
fn view_override_to_write_back(
    open_file: Option<&Path>,
    reading_direction: ReadingDirection,
    spread_mode: gashuu_core::SpreadMode,
    cover_mode: gashuu_core::CoverMode,
    fit_mode: FitMode,
    inherit_pending: bool,
) -> Option<(PathBuf, ViewOverride)> {
    open_file.map(|p| {
        let overrides = if inherit_pending {
            // Keep inheriting: an empty override falls back to every global default.
            ViewOverride::none()
        } else {
            ViewOverride {
                reading_direction: Some(reading_direction),
                spread_mode: Some(spread_mode),
                cover_mode: Some(cover_mode),
                fit_mode: Some(fit_mode),
            }
        };
        (p.to_path_buf(), overrides)
    })
}

/// Write the current runtime view modes back to the OPEN book's override and
/// persist. Reached ONLY via [`route_view_modes_to_sink`] (the routing chokepoint) for
/// the viewer leave/close, open-a-different-book, and exit paths, so a bare
/// keyboard toggle (D/R/C/fit) persists per-book without opening the dialog.
/// No-op when no book is open.
///
/// Borrow discipline (mirrors `write_back_position`): the `state`/`viewport`
/// shared borrows are confined to the leading block expression and drop before
/// `library.borrow_mut()`. `state` and `viewport` are distinct `RefCell`s, so
/// holding shared borrows of both at once is fine.
fn write_back_view_override(
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
            s.is_inherit_pending(),
        )
    }) else {
        return; // no book open — nothing to write back
    };
    library.borrow_mut().set_overrides(&path, overrides);
    if let Err(e) = library.borrow().save() {
        tracing::error!(error = %e, "failed to save library on view-override write-back");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gashuu_core::{CoverMode, SpreadMode};
    use std::path::{Path, PathBuf};

    #[test]
    fn reconcile_writes_runtime_modes_into_settings() {
        // Runtime state is the single source of truth: set the three ViewerState
        // modes and the viewport's fit to NON-default values...
        let mut state = ViewerState::new();
        let _ = state.set_reading_direction(ReadingDirection::Ltr);
        let _ = state.set_spread_mode(SpreadMode::Double);
        let _ = state.set_cover_mode(CoverMode::Paired);
        let mut viewport = ViewportState::from_settings(&Settings::default());
        viewport.set_fit(FitMode::Actual);

        // NON-mirrored fields set to NON-default via struct-update (dodges
        // clippy::field_reassign_with_default) to prove reconcile touches only the four.
        let mut settings = Settings {
            cache_capacity: 99,
            prefetch_radius: 7,
            track_recent_sources: true,
            allow_rar_archives: false,
            ..Settings::default()
        };
        apply_runtime_view_to_settings(&state, &viewport, &mut settings);

        // The four mirrored fields now match the runtime; defaults (Rtl/Auto/Standalone/
        // Width) all differ from the values set above, so this can't pass vacuously.
        assert_eq!(settings.reading_direction, ReadingDirection::Ltr);
        assert_eq!(settings.spread_mode, SpreadMode::Double);
        assert_eq!(settings.cover_mode, CoverMode::Paired);
        assert_eq!(settings.fit_mode, FitMode::Actual);
        // ...and the unrelated persisted fields are left untouched.
        assert_eq!(settings.cache_capacity, 99);
        assert_eq!(settings.prefetch_radius, 7);
        assert!(settings.track_recent_sources);
        assert!(!settings.allow_rar_archives);
    }

    // ---- current_book_name (#71 title-bar) -------------------------------

    #[test]
    fn current_book_name_empty_after_failed_open() {
        // Bug guard: a FAILED open must leave the title-bar name empty. `current_book_name`
        // reads authoritative `open_file()` (None after a failed open), never the dialog path.
        let state = Rc::new(RefCell::new(ViewerState::new()));
        // Sanity: blank before any open.
        assert_eq!(current_book_name(&state), "");
        // A nonexistent path makes `open_path` return Err before `set_source`,
        // so `open_file()` stays None and the derived name stays empty.
        let _ = state
            .borrow_mut()
            .open_path(Path::new("/nonexistent_gashuu_title_guard"));
        assert_eq!(
            current_book_name(&state),
            "",
            "a failed open must not set a title-bar name"
        );
    }

    #[test]
    fn current_book_name_is_folder_name_after_successful_open() {
        // A SUCCESSFUL folder open derives the directory name from canonical `open_file()`.
        // Uses a real temp dir (an empty folder opens fine as a FolderSource).
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
            .open_path(&dir)
            .expect("open_path on a real directory must succeed");
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
        let path = PathBuf::from("/some/book.cbz");
        let result = position_to_write_back(Some(path.as_path()), 7);
        assert!(result.is_some(), "open file => write-back tuple");
        let (p, pg) = result.unwrap();
        assert_eq!(p, path);
        assert_eq!(pg, 7);
    }

    #[test]
    fn position_to_write_back_zero_page() {
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
                false,
            )
            .is_none(),
            "no open file => no write-back"
        );
    }

    #[test]
    fn view_override_to_write_back_some_carries_all_four_modes() {
        let path = PathBuf::from("/manga/book.cbz");
        let result = view_override_to_write_back(
            Some(path.as_path()),
            ReadingDirection::Rtl,
            gashuu_core::SpreadMode::Double,
            gashuu_core::CoverMode::Paired,
            FitMode::Actual,
            false,
        );
        let (p, ov) = result.expect("open file => write-back tuple");
        assert_eq!(p, path);
        assert_eq!(ov.reading_direction, Some(ReadingDirection::Rtl));
        assert_eq!(ov.spread_mode, Some(gashuu_core::SpreadMode::Double));
        assert_eq!(ov.cover_mode, Some(gashuu_core::CoverMode::Paired));
        assert_eq!(ov.fit_mode, Some(FitMode::Actual));
    }

    // ---- inherit-pending guard (#415: reset-to-global undone on close) -----

    #[test]
    fn view_override_to_write_back_inherit_pending_keeps_override_empty() {
        // CX repro: a book is open AND was just "reset to global" (inherit_pending),
        // so the write-back on dialog close must keep the override EMPTY (inherit),
        // NOT re-pin the four current runtime modes.
        let path = PathBuf::from("/manga/book.cbz");
        let result = view_override_to_write_back(
            Some(path.as_path()),
            ReadingDirection::Rtl,
            gashuu_core::SpreadMode::Double,
            gashuu_core::CoverMode::Paired,
            FitMode::Actual,
            true,
        );
        let (p, ov) = result.expect("open file => write-back tuple");
        assert_eq!(p, path);
        assert!(
            ov.is_empty(),
            "an inherit-pending book must persist an EMPTY override (all None), \
             so the reset is not undone on close"
        );
    }

    #[test]
    fn view_override_to_write_back_pins_when_flag_cleared_after_reset() {
        // Regression case: after reset the user changes a mode again, which clears
        // inherit_pending; the write-back must then re-create the override with the
        // four current runtime modes (the guard does not block re-selection).
        let path = PathBuf::from("/manga/book.cbz");
        let (_, ov) = view_override_to_write_back(
            Some(path.as_path()),
            ReadingDirection::Ltr,
            gashuu_core::SpreadMode::Single,
            gashuu_core::CoverMode::Standalone,
            FitMode::Whole,
            false,
        )
        .expect("open file => write-back tuple");
        assert!(!ov.is_empty(), "a cleared flag must pin the runtime modes");
        assert_eq!(ov.reading_direction, Some(ReadingDirection::Ltr));
        assert_eq!(ov.spread_mode, Some(gashuu_core::SpreadMode::Single));
        assert_eq!(ov.cover_mode, Some(gashuu_core::CoverMode::Standalone));
        assert_eq!(ov.fit_mode, Some(FitMode::Whole));
    }
}
