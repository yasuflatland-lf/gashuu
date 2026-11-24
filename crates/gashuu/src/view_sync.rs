use crate::{
    dialog_session::DialogSession, save_library, viewer_state::ViewerState,
    viewport::ViewportState, LibraryStoreHandle,
};
use gashuu_core::{BookId, CoreError, Library, ResolvedView, Settings, ViewOverride};
use std::cell::RefCell;

use std::rc::Rc;

/// The leave/close point at which runtime view modes are persisted, naming WHERE
/// the runtime came from so [`LeavePointService::persist`] can route to the right sink.
/// One variant per production call site of the old write helpers.
#[derive(Clone, Copy, Debug)]
pub(crate) enum ViewModeRoute {
    /// Settings dialog closed on the Library screen (screen 0): the dialog edits
    /// the GLOBAL defaults, so the runtime is reconciled into `Settings`.
    DialogClosedOnLibrary,
    /// Settings dialog closed on the Viewer screen (screen 1): the dialog edits
    /// the CURRENT book's per-book override.
    DialogClosedOnViewer,
    /// Leaving the viewer for the Library (↑): persist the open book's override.
    LeaveViewer,
    /// Opening a different book while one is open (`OpenBookUseCase::apply_opened`):
    /// persist the OUTGOING book's override before the source is replaced.
    OpenDifferentBook,
    /// Desktop app exit: persist the open book's override, then reconcile into the GLOBAL
    /// defaults ONLY when no book is open.
    #[cfg(not(target_arch = "wasm32"))]
    AppExit,
    /// Web tab hidden / page hidden: persist without disturbing live UI state.
    /// Same as `AppExit` when no settings dialog is open; position-only otherwise.
    #[cfg(any(target_arch = "wasm32", test))]
    Suspend,
}

/// The leave-point persistence application service: one instance per app,
/// constructed in `main.rs` and shared by `Rc` with every leave point (the two
/// settings-dialog close branches, the ↑ leave-viewer key, `OpenBookUseCase`,
/// and exit staging).
///
/// It is a SERVICE OBJECT, not a state bundle: the handles below are its own
/// collaborators for this one transaction and are never exposed, so a caller
/// cannot reach through it for unrelated state. This mirrors how
/// `OpenBookUseCase` and `RemoveBooksUseCase` hold their collaborators as
/// fields, replacing the bare procedure that re-took the same five handles at
/// each of its four call sites and grew a parameter on every leave-point change.
pub(crate) struct LeavePointService {
    state: Rc<RefCell<ViewerState>>,
    viewport: Rc<RefCell<ViewportState>>,
    dialog_session: Rc<RefCell<DialogSession>>,
    settings: Rc<RefCell<Settings>>,
    library: Rc<RefCell<Library>>,
    library_store: LibraryStoreHandle,
}

impl LeavePointService {
    pub(crate) fn new(
        state: Rc<RefCell<ViewerState>>,
        viewport: Rc<RefCell<ViewportState>>,
        dialog_session: Rc<RefCell<DialogSession>>,
        settings: Rc<RefCell<Settings>>,
        library: Rc<RefCell<Library>>,
        library_store: LibraryStoreHandle,
    ) -> Self {
        Self {
            state,
            viewport,
            dialog_session,
            settings,
            library,
            library_store,
        }
    }

    /// Production entry point: runs [`Self::persist_with`] with the save the
    /// service was constructed over, i.e. through its own `LibraryStore` handle.
    pub(crate) fn persist(&self, route: ViewModeRoute) -> Result<(), CoreError> {
        self.persist_with(route, |library| save_library(&self.library_store, library))
    }

    /// The ONE place a leave point persists the library: stages the position
    /// write-back (viewer-leaving routes) and the view-mode routing for `route`,
    /// then saves the library at most ONCE. Returns the save result for surfacing.
    /// Settings saves deliberately stay at their call sites (already <= 1 per
    /// event; consolidation evaluated and declined — geometry capture must precede
    /// the exit-time settings save).
    ///
    /// ADR-0007 clobber-trap, made structural here (it once shipped as a real bug):
    /// once view modes became per-book with a global fallback, EVERY "copy runtime →
    /// global" op (`apply_runtime_view_to_settings`) became a potential CLOBBER — the runtime may
    /// hold a per-book value, so reconciling it would overwrite the GLOBAL default
    /// with one book's preference. The routing match below is the invariant: the
    /// GLOBAL sink is written ONLY by (a) the Library-screen settings dialog close and
    /// (b) exit or dialog-free suspend when no book is open. The PER-BOOK sink is written at leave
    /// points (the Viewer-screen settings dialog close, the ↑ leave-viewer key, and
    /// opening a different book while one is open). Note (a): the Library dialog
    /// legitimately reconciles into global even while a book is loaded in
    /// `ViewerState`, because the runtime was global-seeded
    /// by `apply_global_view_to_runtime` at dialog open — so this path must NOT be
    /// blanket-guarded on `open_book().is_none()`, or Library-dialog edits would be
    /// dropped. The exit path keeps the per-book write FIRST, then the open-state
    /// guard on the global reconcile.
    /// Suspend writes the per-book or global sink only when no dialog session is active.
    ///
    /// This is also the SAVE-INJECTION SEAM: `open_book.rs` and `main.rs`'s
    /// `stage_exit_state` call it with their own effect so the one-save boundary
    /// and the write ORDERING are provable without touching the process data
    /// directory. [`Self::persist`] is the thin production wrapper.
    pub(crate) fn persist_with(
        &self,
        route: ViewModeRoute,
        save: impl FnOnce(&Library) -> Result<(), CoreError>,
    ) -> Result<(), CoreError> {
        let writes_position = match route {
            ViewModeRoute::DialogClosedOnLibrary | ViewModeRoute::DialogClosedOnViewer => false,
            ViewModeRoute::LeaveViewer | ViewModeRoute::OpenDifferentBook => true,
            #[cfg(not(target_arch = "wasm32"))]
            ViewModeRoute::AppExit => true,
            #[cfg(any(target_arch = "wasm32", test))]
            ViewModeRoute::Suspend => true,
        };
        if writes_position {
            stage_position_write_back(&self.state, &self.library);
        }
        stage_view_modes_to_sink(
            route,
            &self.state,
            &self.viewport,
            &self.dialog_session,
            &self.settings,
            &self.library,
        );

        let result = save(&self.library.borrow());
        if let Err(e) = &result {
            tracing::error!(error = %e, "failed to save library at leave point");
        }
        result
    }
}

/// Stage the ADR-0007 view-mode sink mutation without performing I/O.
fn stage_view_modes_to_sink(
    route: ViewModeRoute,
    state: &RefCell<ViewerState>,
    viewport: &RefCell<ViewportState>,
    dialog_session: &RefCell<DialogSession>,
    settings: &RefCell<Settings>,
    library: &RefCell<Library>,
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
            stage_view_override_write_back(state, viewport, dialog_session, settings, library);
        }
        #[cfg(not(target_arch = "wasm32"))]
        ViewModeRoute::AppExit => {
            stage_exit_view_modes(state, viewport, dialog_session, settings, library);
        }
        #[cfg(any(target_arch = "wasm32", test))]
        ViewModeRoute::Suspend => {
            if dialog_session.borrow().scope().is_none() {
                stage_exit_view_modes(state, viewport, dialog_session, settings, library);
            }
        }
    }
}

fn stage_exit_view_modes(
    state: &RefCell<ViewerState>,
    viewport: &RefCell<ViewportState>,
    dialog_session: &RefCell<DialogSession>,
    settings: &RefCell<Settings>,
    library: &RefCell<Library>,
) {
    // Per-book override FIRST (no-op if no book is open), so the open
    // book's modes are saved before the open-state-guarded global reconcile.
    stage_view_override_write_back(state, viewport, dialog_session, settings, library);
    if state.borrow().open_book().is_none() {
        apply_runtime_view_to_settings(
            &state.borrow(),
            &viewport.borrow(),
            &mut settings.borrow_mut(),
        );
    }
}

/// Copy the runtime-owned display settings into the persisted `Settings` just
/// before saving. Reached via the ADR-0007 routing chokepoint and by
/// `DialogSession::end`, so both Library-session end paths use the same
/// runtime-to-global reconciliation.
pub(crate) fn apply_runtime_view_to_settings(
    state: &ViewerState,
    viewport: &ViewportState,
    settings: &mut Settings,
) {
    settings.set_view_defaults(runtime_view(state, viewport));
}

/// Snapshot the current runtime view modes as a `ResolvedView`.
///
/// Reads the three `ViewerState`-owned modes (direction/spread/cover) plus the
/// `ViewportState`-owned fit mode. Used by the Library-screen settings dialog
/// (issue #414): the dialog seeds the SHARED runtime with global defaults on
/// open (`apply_global_view_to_runtime`), which would clobber a still-open
/// book's runtime; snapshotting it here lets `on_close_settings` restore the
/// book's own runtime, so the later leave/exit write-back diffs the restored
/// BOOK's value rather than the transiently-global one.
///
/// Borrow discipline: `state` and `viewport` are distinct `RefCell`s, so the
/// two shared borrows never conflict; both drop on return.
pub(crate) fn current_runtime_view(
    state: &RefCell<ViewerState>,
    viewport: &RefCell<ViewportState>,
) -> ResolvedView {
    runtime_view(&state.borrow(), &viewport.borrow())
}

/// Read the view modes from their runtime owners.
pub(crate) fn runtime_view(state: &ViewerState, viewport: &ViewportState) -> ResolvedView {
    ResolvedView {
        reading_direction: state.reading_direction(),
        spread_mode: state.spread_mode(),
        cover_mode: state.cover_mode(),
        fit_mode: viewport.fit_mode(),
    }
}

/// Mirror the GLOBAL `Settings` view modes into the runtime (`ViewerState` for
/// direction/spread/cover, `ViewportState` for fit) — the inverse of
/// `apply_runtime_view_to_settings`. Used when the dialog edits the global defaults
/// (opening Library settings) and when resetting an open book to global.
///
/// The settings borrow drops before the runtime state and viewport are
/// borrowed mutably.
pub(crate) fn apply_global_view_to_runtime(
    settings: &RefCell<Settings>,
    state: &RefCell<ViewerState>,
    viewport: &RefCell<ViewportState>,
) {
    let view = settings.borrow().view_defaults();
    state
        .borrow_mut()
        .apply_resolved_view(view, &mut viewport.borrow_mut());
}

/// The title captured by the open probe for the active book.
pub(crate) fn current_book_name(state: &RefCell<ViewerState>) -> String {
    state.borrow().open_title().unwrap_or_default().to_owned()
}

/// Pure helper: decide if and what to write back to the Library.
///
/// Returns `Some((book_id, page_index))` when a write-back should be
/// performed (a book is open), `None` otherwise. Extracted for table-testing
/// so the predicate can be verified independently of the effectful
/// `stage_position_write_back` that actually calls `library.set_resume_page`.
fn position_to_write_back(open_book: Option<&BookId>, page: usize) -> Option<(BookId, usize)> {
    open_book.map(|p| (p.clone(), page))
}

/// Stage the current reading position in the Library without persisting.
///
/// Called at every leave point: ↑ to Library, opening a different book,
/// and app exit. `set_resume_page` returns `false` when the path is absent or
/// the value is unchanged (idempotent). [`LeavePointService::persist`] performs the
/// single save after every applicable mutation has been staged.
///
/// Borrow discipline: `state` and `library` are distinct `RefCell`s, so
/// borrowing one never affects the other. The opening `let` takes a single
/// shared borrow of `state` and reads both fields from it; that `Ref` drops at
/// the end of the statement, before `library` is borrowed. Each statement's
/// borrows drop before the next statement acquires a different borrow,
/// following the one-statement rule in `docs/patterns.md`.
fn stage_position_write_back(state: &RefCell<ViewerState>, library: &RefCell<Library>) {
    // Extract the (id, page) tuple from the viewer state under one shared
    // borrow; the `Ref` drops at the `;` before `library` is borrowed.
    let Some((id, page)) = ({
        let s = state.borrow();
        position_to_write_back(s.open_book(), s.resume_index_to_persist())
    }) else {
        return; // no book open — nothing to write back
    };
    library.borrow_mut().set_resume_page(&id, page);
}

/// Pure helper: decide what view override to write back for the open book.
///
/// Returns `Some((book_id, override))` when a book is open (so the
/// caller persists it), `None` otherwise. The override carries only runtime modes
/// that differ from `global`; matching fields remain inherited.
///
/// `inherit_pending` is the "Reset to global" guard: when the open book was just
/// reset and no mode has changed since, the write-back must keep the override
/// EMPTY (`ViewOverride::none()`) rather than re-pin the runtime — otherwise
/// closing the dialog would instantly undo the reset. Cleared by any real mode
/// change, because `DialogSession` derives the guard from runtime equality.
///
/// Extracted (mirrors `position_to_write_back`) so the predicate is unit-tested
/// without the effectful `set_overrides` + `save`.
fn view_override_to_write_back(
    open_book: Option<&BookId>,
    view: ResolvedView,
    global: ResolvedView,
    inherit_pending: bool,
) -> Option<(BookId, ViewOverride)> {
    open_book.map(|p| {
        let overrides = if inherit_pending {
            // Keep inheriting: an empty override falls back to every global default.
            ViewOverride::none()
        } else {
            ViewOverride::differences_from(view, global)
        };
        (p.clone(), overrides)
    })
}

/// Write the current runtime view modes back to the OPEN book's override and
/// stage it without persisting. Reached ONLY via [`LeavePointService::persist`] (the
/// routing chokepoint) for the viewer leave/close, open-a-different-book, and exit paths, so a bare
/// keyboard toggle (D/R/C/fit) persists per-book without opening the dialog.
/// No-op when no book is open.
///
/// Borrow discipline (mirrors `stage_position_write_back`): the current runtime
/// view is built in its own statement, then the session predicate and diff are
/// each computed in one statement. All shared borrows drop before
/// `library.borrow_mut()`.
fn stage_view_override_write_back(
    state: &RefCell<ViewerState>,
    viewport: &RefCell<ViewportState>,
    dialog_session: &RefCell<DialogSession>,
    settings: &RefCell<Settings>,
    library: &RefCell<Library>,
) {
    let current = current_runtime_view(state, viewport);
    let inherit_pending = dialog_session.borrow().inherit_pending(current);
    let Some((id, overrides)) = ({
        let s = state.borrow();
        view_override_to_write_back(
            s.open_book(),
            current,
            settings.borrow().view_defaults(),
            inherit_pending,
        )
    }) else {
        return; // no book open — nothing to write back
    };
    library.borrow_mut().set_overrides(&id, overrides);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dialog_session::{DialogScope, DialogSession};
    use gashuu_core::{CoverMode, FitMode, ReadingDirection, SpreadMode};
    use std::path::Path;

    #[test]
    fn apply_global_view_to_runtime_installs_view_defaults() {
        let initial = Settings::default();
        let state = RefCell::new(ViewerState::from_settings(&initial));
        let viewport = RefCell::new(ViewportState::from_settings(&initial));
        let globals = Settings {
            reading_direction: ReadingDirection::Ltr,
            spread_mode: SpreadMode::Double,
            cover_mode: CoverMode::Paired,
            fit_mode: FitMode::Actual,
            ..Settings::default()
        };
        let expected = globals.view_defaults();

        apply_global_view_to_runtime(&RefCell::new(globals), &state, &viewport);

        assert_eq!(current_runtime_view(&state, &viewport), expected);
    }

    #[test]
    fn apply_runtime_view_to_settings_writes_all_four_fields() {
        let mut settings = Settings::default();
        let mut state = ViewerState::from_settings(&settings);
        let mut viewport = ViewportState::from_settings(&settings);
        let runtime = ResolvedView {
            reading_direction: ReadingDirection::Ltr,
            spread_mode: SpreadMode::Double,
            cover_mode: CoverMode::Paired,
            fit_mode: FitMode::Actual,
        };
        state.apply_resolved_view(runtime, &mut viewport);

        apply_runtime_view_to_settings(&state, &viewport, &mut settings);

        assert_eq!(settings.view_defaults(), runtime);
    }

    fn leave_point_service(
        state: &Rc<RefCell<ViewerState>>,
        viewport: &Rc<RefCell<ViewportState>>,
        dialog_session: &Rc<RefCell<DialogSession>>,
        settings: &Rc<RefCell<Settings>>,
        library: &Rc<RefCell<Library>>,
    ) -> LeavePointService {
        LeavePointService::new(
            Rc::clone(state),
            Rc::clone(viewport),
            Rc::clone(dialog_session),
            Rc::clone(settings),
            Rc::clone(library),
            Rc::new(None),
        )
    }

    struct SuspendFixture {
        _dir: tempfile::TempDir,
        book_id: BookId,
        state: Rc<RefCell<ViewerState>>,
        viewport: Rc<RefCell<ViewportState>>,
        session: Rc<RefCell<DialogSession>>,
        settings: Rc<RefCell<Settings>>,
        library: Rc<RefCell<Library>>,
    }

    impl SuspendFixture {
        fn new() -> Self {
            let dir = tempfile::tempdir().unwrap();
            for page in 0..10 {
                std::fs::write(dir.path().join(format!("{page}.png")), []).unwrap();
            }
            let settings = Rc::new(RefCell::new(Settings::default()));
            let state = Rc::new(RefCell::new(ViewerState::from_settings(&settings.borrow())));
            state.borrow_mut().open_path(dir.path()).unwrap();
            assert!(state.borrow_mut().jump_to(7));
            state
                .borrow_mut()
                .set_reading_direction(ReadingDirection::Ltr);
            let book_id = state.borrow().open_book().unwrap().clone();
            let viewport = Rc::new(RefCell::new(ViewportState::from_settings(
                &settings.borrow(),
            )));
            let session = Rc::new(RefCell::new(DialogSession::new()));
            let mut value = Library::new();
            assert!(value.add(book_id.clone(), "book".into()).is_some());
            value.set_overrides(
                &book_id,
                ViewOverride::differences_from(
                    current_runtime_view(&state, &viewport),
                    settings.borrow().view_defaults(),
                ),
            );
            Self {
                _dir: dir,
                book_id,
                state,
                viewport,
                session,
                settings,
                library: Rc::new(RefCell::new(value)),
            }
        }
        fn persist(&self, route: ViewModeRoute) -> String {
            let mut saved = String::new();
            leave_point_service(
                &self.state,
                &self.viewport,
                &self.session,
                &self.settings,
                &self.library,
            )
            .persist_with(route, |library| {
                saved = library.to_json().unwrap();
                Ok(())
            })
            .unwrap();
            saved
        }
        fn open_dialog(&self, scope: DialogScope) {
            self.session
                .borrow_mut()
                .open(scope, &self.state, &self.viewport, &self.settings);
        }
    }

    #[test]
    fn suspend_without_dialog_matches_app_exit() {
        let f = SuspendFixture::new();
        f.state.borrow_mut().set_spread_mode(SpreadMode::Single);
        let before = f.library.borrow().to_json().unwrap();
        let suspend = f.persist(ViewModeRoute::Suspend);
        let settings = f.settings.borrow().view_defaults();
        *f.library.borrow_mut() = Library::from_json(&before).unwrap();
        assert_eq!(suspend, f.persist(ViewModeRoute::AppExit));
        assert_eq!(settings, f.settings.borrow().view_defaults());
        assert_eq!(f.library.borrow().resume_page(&f.book_id), 7);
    }

    fn assert_suspend_dialog(scope: DialogScope) {
        let f = SuspendFixture::new();
        let overrides = f.library.borrow().overrides_for(&f.book_id);
        let settings = f.settings.borrow().view_defaults();
        f.open_dialog(scope);
        if scope == DialogScope::Viewer {
            f.state.borrow_mut().set_spread_mode(SpreadMode::Single);
        }
        let runtime = current_runtime_view(&f.state, &f.viewport);
        let saved = f.persist(ViewModeRoute::Suspend);
        let saved = Library::from_json(&saved).unwrap();
        assert_eq!(saved.resume_page(&f.book_id), 7);
        assert_eq!(saved.overrides_for(&f.book_id), overrides);
        assert_eq!(f.settings.borrow().view_defaults(), settings);
        assert_eq!(f.session.borrow().scope(), Some(scope));
        assert_eq!(current_runtime_view(&f.state, &f.viewport), runtime);
        if scope == DialogScope::Library {
            assert_eq!(runtime, settings);
        }
    }

    #[test]
    fn suspend_with_library_dialog_writes_position_only() {
        assert_suspend_dialog(DialogScope::Library);
    }
    #[test]
    fn suspend_with_viewer_dialog_writes_position_only() {
        assert_suspend_dialog(DialogScope::Viewer);
    }

    #[test]
    fn suspend_then_dialog_close_equals_dialog_close_alone() {
        let f = SuspendFixture::new();
        let initial = f.library.borrow().to_json().unwrap();
        f.open_dialog(DialogScope::Library);
        f.state.borrow_mut().set_spread_mode(SpreadMode::Single);
        f.persist(ViewModeRoute::Suspend);
        f.session
            .borrow_mut()
            .end(&f.state, &f.viewport, &f.settings);
        f.persist(ViewModeRoute::DialogClosedOnLibrary);
        let overrides = f.library.borrow().overrides_for(&f.book_id);
        let settings = f.settings.borrow().view_defaults();
        assert_eq!(f.library.borrow().resume_page(&f.book_id), 7);
        *f.library.borrow_mut() = Library::from_json(&initial).unwrap();
        *f.settings.borrow_mut() = Settings::default();
        f.open_dialog(DialogScope::Library);
        f.state.borrow_mut().set_spread_mode(SpreadMode::Single);
        f.session
            .borrow_mut()
            .end(&f.state, &f.viewport, &f.settings);
        f.persist(ViewModeRoute::DialogClosedOnLibrary);
        assert_eq!(f.library.borrow().overrides_for(&f.book_id), overrides);
        assert_eq!(f.settings.borrow().view_defaults(), settings);
        assert_eq!(f.library.borrow().resume_page(&f.book_id), 0);
    }

    #[test]
    fn suspend_is_idempotent() {
        let f = SuspendFixture::new();
        assert_eq!(
            f.persist(ViewModeRoute::Suspend),
            f.persist(ViewModeRoute::Suspend)
        );
    }

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
        // reads authoritative `open_book()` (None after a failed open), never the dialog path.
        let state = Rc::new(RefCell::new(ViewerState::new()));
        // Sanity: blank before any open.
        assert_eq!(current_book_name(&state), "");
        // A nonexistent path makes `open_path` return Err before `set_source`,
        // so `open_book()` stays None and the derived name stays empty.
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
        // A SUCCESSFUL folder open derives the directory name from canonical `open_book()`.
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
    fn position_to_write_back_none_when_no_open_book() {
        assert!(
            position_to_write_back(None, 5).is_none(),
            "no open file => no write-back"
        );
    }

    #[test]
    fn position_to_write_back_some_when_file_open() {
        let path = BookId::new("/some/book.cbz");
        let result = position_to_write_back(Some(&path), 7);
        assert!(result.is_some(), "open file => write-back tuple");
        let (p, pg) = result.unwrap();
        assert_eq!(p, path);
        assert_eq!(pg, 7);
    }

    #[test]
    fn position_to_write_back_zero_page() {
        let path = BookId::new("/some/book.cbz");
        let result = position_to_write_back(Some(&path), 0);
        assert!(result.is_some());
        let (_, pg) = result.unwrap();
        assert_eq!(pg, 0, "page 0 is a valid write-back (start of book)");
    }

    #[test]
    fn staged_write_back_persists_finished_index_for_final_double_paired_spread() {
        let root = tempfile::tempdir().expect("tempdir");
        let book = root.path().join("book");
        std::fs::create_dir(&book).expect("create book");
        for page in 0..10 {
            std::fs::write(book.join(format!("{page}.png")), []).expect("write page");
        }

        let settings = Settings {
            spread_mode: SpreadMode::Double,
            cover_mode: CoverMode::Paired,
            ..Settings::default()
        };
        let mut runtime = ViewerState::from_settings(&settings);
        runtime.open_path(&book).expect("open test book");
        assert!(runtime.jump_to(8));
        assert_eq!(runtime.index(), 8);
        let canonical = runtime
            .open_book()
            .expect("open file after successful open")
            .clone();

        let mut library_value = Library::new();
        assert!(library_value
            .add(canonical.clone(), "book".into())
            .is_some());
        let state = Rc::new(RefCell::new(runtime));
        let library = Rc::new(RefCell::new(library_value));

        stage_position_write_back(&state, &library);

        assert_eq!(library.borrow().resume_page(&canonical), 9);
    }

    // ---- view_override_to_write_back (per-book overrides) ------------------

    #[test]
    fn leaving_a_book_at_the_global_defaults_writes_no_override() {
        let root = tempfile::tempdir().expect("tempdir");
        let book = root.path().join("book");
        std::fs::create_dir(&book).expect("create book");

        let global = Settings {
            reading_direction: ReadingDirection::Ltr,
            spread_mode: SpreadMode::Double,
            cover_mode: CoverMode::Paired,
            fit_mode: FitMode::Actual,
            ..Settings::default()
        };
        let mut runtime = ViewerState::from_settings(&global);
        runtime.open_path(&book).expect("open test book");
        let canonical = runtime
            .open_book()
            .expect("open file after successful open")
            .clone();
        let state = Rc::new(RefCell::new(runtime));
        let viewport = Rc::new(RefCell::new(ViewportState::from_settings(&global)));
        let settings = Rc::new(RefCell::new(global));
        let mut library_value = Library::new();
        assert!(library_value
            .add(canonical.clone(), "book".into())
            .is_some());
        let library = Rc::new(RefCell::new(library_value));
        let dialog_session = Rc::new(RefCell::new(DialogSession::new()));

        leave_point_service(&state, &viewport, &dialog_session, &settings, &library)
            .persist_with(ViewModeRoute::LeaveViewer, |_| Ok(()))
            .expect("persist leave point");

        let staged = library.borrow().overrides_for(&canonical);
        assert!(
            staged.is_empty(),
            "leaving without changing the global-seeded runtime must keep every field inherited; \
             got {staged:?}"
        );
        let json = library.borrow().to_json().expect("serialize library");
        assert!(
            !json.contains("\"overrides\""),
            "an empty override must not emit an overrides key: {json}"
        );
    }

    #[test]
    fn a_book_at_the_global_value_follows_a_later_global_change() {
        let root = tempfile::tempdir().expect("tempdir");
        let book = root.path().join("book");
        std::fs::create_dir(&book).expect("create book");

        let initial_global = Settings {
            reading_direction: ReadingDirection::Rtl,
            spread_mode: SpreadMode::Double,
            cover_mode: CoverMode::Paired,
            fit_mode: FitMode::Actual,
            ..Settings::default()
        };
        let mut runtime = ViewerState::from_settings(&initial_global);
        runtime.open_path(&book).expect("open test book");
        let canonical = runtime
            .open_book()
            .expect("open file after successful open")
            .clone();
        let state = Rc::new(RefCell::new(runtime));
        let viewport = Rc::new(RefCell::new(ViewportState::from_settings(&initial_global)));
        let settings = Rc::new(RefCell::new(initial_global));
        let mut library_value = Library::new();
        assert!(library_value
            .add(canonical.clone(), "book".into())
            .is_some());
        let library = Rc::new(RefCell::new(library_value));
        let dialog_session = Rc::new(RefCell::new(DialogSession::new()));

        leave_point_service(&state, &viewport, &dialog_session, &settings, &library)
            .persist_with(ViewModeRoute::LeaveViewer, |_| Ok(()))
            .expect("persist leave point");
        assert!(library.borrow().overrides_for(&canonical).is_empty());

        *settings.borrow_mut() = Settings {
            reading_direction: ReadingDirection::Ltr,
            spread_mode: SpreadMode::Single,
            cover_mode: CoverMode::Standalone,
            fit_mode: FitMode::Whole,
            ..Settings::default()
        };
        // Opening a book resolves its stored override against the settings in
        // force at that time; exercise that same reopen boundary directly.
        let reopened = library
            .borrow()
            .overrides_for(&canonical)
            .resolve(settings.borrow().view_defaults());

        assert_eq!(
            reopened,
            ResolvedView {
                reading_direction: ReadingDirection::Ltr,
                spread_mode: SpreadMode::Single,
                cover_mode: CoverMode::Standalone,
                fit_mode: FitMode::Whole,
            }
        );
    }

    #[test]
    fn an_explicitly_different_mode_survives_a_global_change() {
        let root = tempfile::tempdir().expect("tempdir");
        let book = root.path().join("book");
        std::fs::create_dir(&book).expect("create book");

        let initial_global = Settings {
            reading_direction: ReadingDirection::Rtl,
            spread_mode: SpreadMode::Double,
            cover_mode: CoverMode::Paired,
            fit_mode: FitMode::Actual,
            ..Settings::default()
        };
        let mut runtime = ViewerState::from_settings(&initial_global);
        runtime.set_spread_mode(SpreadMode::Single);
        runtime.open_path(&book).expect("open test book");
        let canonical = runtime
            .open_book()
            .expect("open file after successful open")
            .clone();
        let state = Rc::new(RefCell::new(runtime));
        let viewport = Rc::new(RefCell::new(ViewportState::from_settings(&initial_global)));
        let settings = Rc::new(RefCell::new(initial_global));
        let mut library_value = Library::new();
        assert!(library_value
            .add(canonical.clone(), "book".into())
            .is_some());
        let library = Rc::new(RefCell::new(library_value));
        let dialog_session = Rc::new(RefCell::new(DialogSession::new()));

        leave_point_service(&state, &viewport, &dialog_session, &settings, &library)
            .persist_with(ViewModeRoute::LeaveViewer, |_| Ok(()))
            .expect("persist leave point");
        assert_eq!(
            library.borrow().overrides_for(&canonical),
            ViewOverride {
                reading_direction: None,
                spread_mode: Some(SpreadMode::Single),
                cover_mode: None,
                fit_mode: None,
            }
        );

        *settings.borrow_mut() = Settings {
            reading_direction: ReadingDirection::Ltr,
            spread_mode: SpreadMode::Auto,
            cover_mode: CoverMode::Standalone,
            fit_mode: FitMode::Whole,
            ..Settings::default()
        };
        let reopened = library
            .borrow()
            .overrides_for(&canonical)
            .resolve(settings.borrow().view_defaults());

        assert_eq!(
            reopened,
            ResolvedView {
                reading_direction: ReadingDirection::Ltr,
                spread_mode: SpreadMode::Single,
                cover_mode: CoverMode::Standalone,
                fit_mode: FitMode::Whole,
            },
            "the explicit spread survives while inherited fields follow the new global"
        );
    }

    #[test]
    fn view_override_to_write_back_none_when_no_open_book() {
        let global = Settings::default();
        assert!(
            view_override_to_write_back(
                None,
                ResolvedView {
                    reading_direction: ReadingDirection::Ltr,
                    spread_mode: SpreadMode::Double,
                    cover_mode: CoverMode::Paired,
                    fit_mode: FitMode::Actual,
                },
                global.view_defaults(),
                false,
            )
            .is_none(),
            "no open file => no write-back"
        );
    }

    #[test]
    fn view_override_to_write_back_some_carries_all_four_modes() {
        let path = BookId::new("/manga/book.cbz");
        let global = Settings {
            reading_direction: ReadingDirection::Ltr,
            spread_mode: SpreadMode::Single,
            cover_mode: CoverMode::Standalone,
            fit_mode: FitMode::Whole,
            ..Settings::default()
        };
        let result = view_override_to_write_back(
            Some(&path),
            ResolvedView {
                reading_direction: ReadingDirection::Rtl,
                spread_mode: SpreadMode::Double,
                cover_mode: CoverMode::Paired,
                fit_mode: FitMode::Actual,
            },
            global.view_defaults(),
            false,
        );
        let (p, ov) = result.expect("open file => write-back tuple");
        assert_eq!(p, path);
        assert_eq!(ov.reading_direction, Some(ReadingDirection::Rtl));
        assert_eq!(ov.spread_mode, Some(gashuu_core::SpreadMode::Double));
        assert_eq!(ov.cover_mode, Some(gashuu_core::CoverMode::Paired));
        assert_eq!(ov.fit_mode, Some(FitMode::Actual));
    }

    #[test]
    fn write_back_emits_only_fields_that_differ_from_global() {
        let path = BookId::new("/manga/book.cbz");
        let global = Settings {
            reading_direction: ReadingDirection::Rtl,
            spread_mode: SpreadMode::Double,
            cover_mode: CoverMode::Standalone,
            fit_mode: FitMode::Actual,
            ..Settings::default()
        };
        let (_, overrides) = view_override_to_write_back(
            Some(&path),
            ResolvedView {
                reading_direction: ReadingDirection::Ltr,
                spread_mode: SpreadMode::Double,
                cover_mode: CoverMode::Paired,
                fit_mode: FitMode::Actual,
            },
            global.view_defaults(),
            false,
        )
        .expect("open file => write-back tuple");

        assert_eq!(
            overrides,
            ViewOverride {
                reading_direction: Some(ReadingDirection::Ltr),
                spread_mode: None,
                cover_mode: Some(CoverMode::Paired),
                fit_mode: None,
            }
        );
    }

    // ---- inherit-pending guard (#415: reset-to-global undone on close) -----

    #[test]
    fn view_override_write_back_returns_empty_while_inherit_pending_even_when_view_differs() {
        // CX repro: a book is open AND was just "reset to global" (inherit_pending),
        // so the write-back on dialog close must keep the override EMPTY (inherit),
        // even though every current runtime mode differs from global.
        let path = BookId::new("/manga/book.cbz");
        let global = Settings {
            reading_direction: ReadingDirection::Ltr,
            spread_mode: SpreadMode::Single,
            cover_mode: CoverMode::Standalone,
            fit_mode: FitMode::Whole,
            ..Settings::default()
        };
        let view = ResolvedView {
            reading_direction: ReadingDirection::Rtl,
            spread_mode: SpreadMode::Double,
            cover_mode: CoverMode::Paired,
            fit_mode: FitMode::Actual,
        };
        let global = global.view_defaults();
        assert_eq!(
            view_override_to_write_back(Some(&path), view, global, true),
            Some((path.clone(), ViewOverride::none()))
        );
        assert_eq!(
            view_override_to_write_back(Some(&path), view, global, false),
            Some((
                path,
                ViewOverride {
                    reading_direction: Some(ReadingDirection::Rtl),
                    spread_mode: Some(SpreadMode::Double),
                    cover_mode: Some(CoverMode::Paired),
                    fit_mode: Some(FitMode::Actual),
                }
            ))
        );
    }

    #[test]
    fn view_override_to_write_back_pins_when_flag_cleared_after_reset() {
        // Regression case: after reset the user changes a mode again, which clears
        // inherit_pending; the write-back must then re-create the differing field
        // (the guard does not block re-selection).
        let path = BookId::new("/manga/book.cbz");
        let global = Settings {
            reading_direction: ReadingDirection::Rtl,
            spread_mode: SpreadMode::Single,
            cover_mode: CoverMode::Standalone,
            fit_mode: FitMode::Whole,
            ..Settings::default()
        };
        let (_, ov) = view_override_to_write_back(
            Some(&path),
            ResolvedView {
                reading_direction: ReadingDirection::Ltr,
                spread_mode: SpreadMode::Single,
                cover_mode: CoverMode::Standalone,
                fit_mode: FitMode::Whole,
            },
            global.view_defaults(),
            false,
        )
        .expect("open file => write-back tuple");
        assert!(!ov.is_empty(), "a cleared flag must pin the runtime modes");
        assert_eq!(ov.reading_direction, Some(ReadingDirection::Ltr));
        assert_eq!(ov.spread_mode, None);
        assert_eq!(ov.cover_mode, None);
        assert_eq!(ov.fit_mode, None);
    }

    #[test]
    fn write_back_after_reset_then_fit_change_pins_the_runtime() {
        let root = tempfile::tempdir().expect("tempdir");
        let book = root.path().join("book");
        std::fs::create_dir(&book).expect("create book");

        let settings = Rc::new(RefCell::new(Settings {
            reading_direction: ReadingDirection::Rtl,
            spread_mode: SpreadMode::Double,
            cover_mode: CoverMode::Paired,
            fit_mode: FitMode::Actual,
            ..Settings::default()
        }));
        let state = Rc::new(RefCell::new(ViewerState::new()));
        state.borrow_mut().open_path(&book).expect("open test book");
        let canonical = state
            .borrow()
            .open_book()
            .expect("open file after successful open")
            .clone();
        let viewport = Rc::new(RefCell::new(ViewportState::from_settings(
            &settings.borrow(),
        )));
        let mut library_value = Library::new();
        assert!(library_value
            .add(canonical.clone(), "book".into())
            .is_some());
        let library = Rc::new(RefCell::new(library_value));
        let dialog_session = Rc::new(RefCell::new(DialogSession::new()));

        dialog_session
            .borrow_mut()
            .reset_to_global(&state, &viewport, &settings);
        viewport.borrow_mut().set_fit(FitMode::Whole);
        leave_point_service(&state, &viewport, &dialog_session, &settings, &library)
            .persist_with(ViewModeRoute::LeaveViewer, |_| Ok(()))
            .expect("persist leave point");

        assert_eq!(
            library.borrow().overrides_for(&canonical),
            ViewOverride {
                // Reset installed the globals and the fit change disarmed its
                // guard, so only that choice is persisted and survives reopen.
                reading_direction: None,
                spread_mode: None,
                cover_mode: None,
                fit_mode: Some(FitMode::Whole),
            }
        );
    }

    #[test]
    fn write_back_right_after_reset_is_empty() {
        let root = tempfile::tempdir().expect("tempdir");
        let book = root.path().join("book");
        std::fs::create_dir(&book).expect("create book");

        let settings = Rc::new(RefCell::new(Settings {
            reading_direction: ReadingDirection::Rtl,
            spread_mode: SpreadMode::Double,
            cover_mode: CoverMode::Paired,
            fit_mode: FitMode::Actual,
            ..Settings::default()
        }));
        let state = Rc::new(RefCell::new(ViewerState::new()));
        state.borrow_mut().open_path(&book).expect("open test book");
        let canonical = state
            .borrow()
            .open_book()
            .expect("open file after successful open")
            .clone();
        let viewport = Rc::new(RefCell::new(ViewportState::from_settings(
            &settings.borrow(),
        )));
        let mut library_value = Library::new();
        assert!(library_value
            .add(canonical.clone(), "book".into())
            .is_some());
        let library = Rc::new(RefCell::new(library_value));
        let dialog_session = Rc::new(RefCell::new(DialogSession::new()));

        dialog_session
            .borrow_mut()
            .reset_to_global(&state, &viewport, &settings);
        leave_point_service(&state, &viewport, &dialog_session, &settings, &library)
            .persist_with(ViewModeRoute::LeaveViewer, |_| Ok(()))
            .expect("persist leave point");

        // Pins the observable outcome; the helper unit test pins the guard itself.
        assert_eq!(
            library.borrow().overrides_for(&canonical),
            ViewOverride::none()
        );
    }

    #[test]
    fn library_dialog_global_edit_after_reset_keeps_override_empty_on_leave() {
        // #414/#415 end-to-end: after a reset, editing a GLOBAL default from the
        // Library-screen dialog must not re-pin the book. The session snapshot
        // restores the reset-installed runtime at `end()`, so the derived
        // predicate still matches and the override stays EMPTY.
        let root = tempfile::tempdir().expect("tempdir");
        let book = root.path().join("book");
        std::fs::create_dir(&book).expect("create book");

        let settings = Rc::new(RefCell::new(Settings {
            reading_direction: ReadingDirection::Rtl,
            spread_mode: SpreadMode::Double,
            cover_mode: CoverMode::Paired,
            fit_mode: FitMode::Actual,
            ..Settings::default()
        }));
        let state = Rc::new(RefCell::new(ViewerState::new()));
        state.borrow_mut().open_path(&book).expect("open test book");
        let canonical = state
            .borrow()
            .open_book()
            .expect("open file after successful open")
            .clone();
        let viewport = Rc::new(RefCell::new(ViewportState::from_settings(
            &settings.borrow(),
        )));
        let mut library_value = Library::new();
        assert!(library_value
            .add(canonical.clone(), "book".into())
            .is_some());
        let library = Rc::new(RefCell::new(library_value));
        let dialog_session = Rc::new(RefCell::new(DialogSession::new()));

        dialog_session
            .borrow_mut()
            .reset_to_global(&state, &viewport, &settings);
        dialog_session
            .borrow_mut()
            .open(DialogScope::Library, &state, &viewport, &settings);
        state.borrow_mut().set_spread_mode(SpreadMode::Single);
        dialog_session
            .borrow_mut()
            .end(&state, &viewport, &settings);
        leave_point_service(&state, &viewport, &dialog_session, &settings, &library)
            .persist_with(ViewModeRoute::LeaveViewer, |_| Ok(()))
            .expect("persist leave point");

        assert_eq!(
            library.borrow().overrides_for(&canonical),
            ViewOverride::none()
        );
    }

    #[test]
    fn library_close_writes_global_once_and_idempotently() {
        let root = tempfile::tempdir().expect("tempdir");
        let book = root.path().join("book");
        std::fs::create_dir(&book).expect("create book");
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
        let state = Rc::new(RefCell::new(ViewerState::new()));
        state.borrow_mut().open_path(&book).expect("open test book");
        let viewport = Rc::new(RefCell::new(ViewportState::from_settings(
            &settings.borrow(),
        )));
        state
            .borrow_mut()
            .apply_resolved_view(book_view, &mut viewport.borrow_mut());
        let mut library_value = Library::new();
        let canonical = state
            .borrow()
            .open_book()
            .expect("open file after successful open")
            .clone();
        assert!(library_value.add(canonical, "book".into()).is_some());
        let library = Rc::new(RefCell::new(library_value));
        let dialog_session = Rc::new(RefCell::new(DialogSession::new()));

        dialog_session
            .borrow_mut()
            .open(DialogScope::Library, &state, &viewport, &settings);
        state
            .borrow_mut()
            .apply_resolved_view(edited_global, &mut viewport.borrow_mut());
        leave_point_service(&state, &viewport, &dialog_session, &settings, &library)
            .persist_with(ViewModeRoute::DialogClosedOnLibrary, |_| Ok(()))
            .expect("persist library dialog close");
        dialog_session
            .borrow_mut()
            .end(&state, &viewport, &settings);

        let settings = settings.borrow();
        assert_eq!(settings.reading_direction, edited_global.reading_direction);
        assert_eq!(settings.spread_mode, edited_global.spread_mode);
        assert_eq!(settings.cover_mode, edited_global.cover_mode);
        assert_eq!(settings.fit_mode, edited_global.fit_mode);
        assert_eq!(current_runtime_view(&state, &viewport), book_view);
    }

    #[test]
    fn exit_with_library_dialog_open_persists_the_books_own_modes() {
        let root = tempfile::tempdir().expect("tempdir");
        let book = root.path().join("book");
        std::fs::create_dir(&book).expect("create book");

        let settings = Rc::new(RefCell::new(Settings {
            reading_direction: ReadingDirection::Rtl,
            spread_mode: SpreadMode::Double,
            cover_mode: CoverMode::Paired,
            fit_mode: FitMode::Actual,
            ..Settings::default()
        }));
        let state = Rc::new(RefCell::new(ViewerState::new()));
        state.borrow_mut().open_path(&book).expect("open test book");
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

        dialog_session
            .borrow_mut()
            .open(DialogScope::Library, &state, &viewport, &settings);
        dialog_session
            .borrow_mut()
            .end(&state, &viewport, &settings);
        leave_point_service(&state, &viewport, &dialog_session, &settings, &library)
            .persist_with(ViewModeRoute::AppExit, |_| Ok(()))
            .expect("persist leave point");

        assert_eq!(
            library.borrow().overrides_for(&canonical),
            ViewOverride {
                reading_direction: Some(ReadingDirection::Ltr),
                spread_mode: Some(SpreadMode::Single),
                cover_mode: Some(CoverMode::Standalone),
                fit_mode: Some(FitMode::Whole),
            }
        );
    }

    #[test]
    fn leave_point_service_saves_once_and_preserves_route_matrix() {
        let routes = [
            ViewModeRoute::DialogClosedOnLibrary,
            ViewModeRoute::DialogClosedOnViewer,
            ViewModeRoute::LeaveViewer,
            ViewModeRoute::OpenDifferentBook,
            ViewModeRoute::AppExit,
        ];

        for route in routes {
            for book_open in [false, true] {
                let root = tempfile::tempdir().expect("tempdir");
                let book = root.path().join("book");
                std::fs::create_dir(&book).expect("create book");
                for page in 0..3 {
                    std::fs::write(book.join(format!("{page}.png")), []).expect("write page");
                }

                let mut runtime = ViewerState::new();
                let mut library_value = Library::new();
                let canonical = if book_open {
                    runtime.open_path(&book).expect("open test book");
                    runtime.jump_to(2);
                    let canonical = runtime
                        .open_book()
                        .expect("open file after successful open")
                        .clone();
                    assert!(library_value
                        .add(canonical.clone(), "book".into())
                        .is_some());
                    Some(canonical)
                } else {
                    None
                };
                runtime.set_reading_direction(ReadingDirection::Ltr);
                runtime.set_spread_mode(SpreadMode::Single);
                runtime.set_cover_mode(CoverMode::Standalone);

                let initial_settings = Settings {
                    reading_direction: ReadingDirection::Rtl,
                    spread_mode: SpreadMode::Double,
                    cover_mode: CoverMode::Paired,
                    fit_mode: FitMode::Actual,
                    ..Settings::default()
                };
                let state = Rc::new(RefCell::new(runtime));
                let settings = Rc::new(RefCell::new(initial_settings));
                let viewport = Rc::new(RefCell::new(ViewportState::from_settings(
                    &settings.borrow(),
                )));
                viewport.borrow_mut().set_fit(FitMode::Whole);
                let library = Rc::new(RefCell::new(library_value));
                let dialog_session = Rc::new(RefCell::new(DialogSession::new()));
                let save_count = std::cell::Cell::new(0);

                let result =
                    leave_point_service(&state, &viewport, &dialog_session, &settings, &library)
                        .persist_with(route, |_| {
                            save_count.set(save_count.get() + 1);
                            Ok(())
                        });

                assert!(result.is_ok());
                assert_eq!(
                    save_count.get(),
                    1,
                    "{route:?}, book_open={book_open}: exactly one save"
                );

                let writes_global = matches!(route, ViewModeRoute::DialogClosedOnLibrary)
                    || matches!(route, ViewModeRoute::AppExit) && !book_open;
                let settings = settings.borrow();
                assert_eq!(
                    settings.reading_direction,
                    if writes_global {
                        ReadingDirection::Ltr
                    } else {
                        ReadingDirection::Rtl
                    },
                    "{route:?}, book_open={book_open}: global direction"
                );
                assert_eq!(
                    settings.spread_mode,
                    if writes_global {
                        SpreadMode::Single
                    } else {
                        SpreadMode::Double
                    },
                    "{route:?}, book_open={book_open}: global spread"
                );
                assert_eq!(
                    settings.cover_mode,
                    if writes_global {
                        CoverMode::Standalone
                    } else {
                        CoverMode::Paired
                    },
                    "{route:?}, book_open={book_open}: global cover"
                );
                assert_eq!(
                    settings.fit_mode,
                    if writes_global {
                        FitMode::Whole
                    } else {
                        FitMode::Actual
                    },
                    "{route:?}, book_open={book_open}: global fit"
                );
                drop(settings);

                if let Some(canonical) = canonical {
                    let library = library.borrow();
                    let writes_position = matches!(
                        route,
                        ViewModeRoute::LeaveViewer
                            | ViewModeRoute::OpenDifferentBook
                            | ViewModeRoute::AppExit
                    );
                    assert_eq!(
                        library.resume_page(&canonical),
                        if writes_position { 2 } else { 0 },
                        "{route:?}: position sink"
                    );

                    let writes_override = !matches!(route, ViewModeRoute::DialogClosedOnLibrary);
                    let overrides = library.overrides_for(&canonical);
                    assert_eq!(
                        overrides.reading_direction,
                        writes_override.then_some(ReadingDirection::Ltr),
                        "{route:?}: per-book direction"
                    );
                    assert_eq!(
                        overrides.spread_mode,
                        writes_override.then_some(SpreadMode::Single),
                        "{route:?}: per-book spread"
                    );
                    assert_eq!(
                        overrides.cover_mode,
                        writes_override.then_some(CoverMode::Standalone),
                        "{route:?}: per-book cover"
                    );
                    assert_eq!(
                        overrides.fit_mode,
                        writes_override.then_some(FitMode::Whole),
                        "{route:?}: per-book fit"
                    );
                }
            }
        }
    }
}
