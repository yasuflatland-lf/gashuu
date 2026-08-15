use crate::view_sync::{apply_global_view_to_runtime, current_runtime_view};
use crate::{viewer_state::ViewerState, viewport::ViewportState};
use gashuu_core::{ResolvedView, Settings};
use std::cell::RefCell;
use std::rc::Rc;

/// The open book's pre-dialog runtime, captured at library-screen dialog open.
struct RuntimeSnapshot {
    view: ResolvedView,
    inherit_pending: bool,
}

/// The screen the settings dialog was OPENED on, recorded once at open. Every
/// side effect of the session routes by this, never by the screen at close: the
/// open sets up global-seeding (Library) or nothing (Viewer), so a close that
/// read a different screen could not undo what the open did (issue #535).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DialogScope {
    /// Screen 0: the dialog edits the GLOBAL defaults through a global-seeded
    /// runtime scratchpad, so the open book's runtime is snapshotted first.
    Library,
    /// Screen 1: the dialog edits the CURRENT book's override, and the runtime
    /// already holds that book's resolved modes — nothing is seeded or saved.
    Viewer,
}

/// The settings-dialog session over its recorded opening scope: owns the open
/// book's pre-dialog runtime snapshot (#414) and reset-to-global ordering (#415).
pub(crate) struct DialogSession {
    scope: Option<DialogScope>,
    snapshot: Option<RuntimeSnapshot>,
}

impl DialogSession {
    pub fn new() -> Self {
        Self {
            scope: None,
            snapshot: None,
        }
    }

    /// Start a dialog session in its opening scope. Library scope snapshots the
    /// open book's runtime, then seeds the runtime with globals; Viewer scope
    /// records the scope without changing the runtime.
    pub fn open(
        &mut self,
        scope: DialogScope,
        state: &Rc<RefCell<ViewerState>>,
        viewport: &Rc<RefCell<ViewportState>>,
        settings: &Rc<RefCell<Settings>>,
    ) {
        self.scope = Some(scope);
        if scope == DialogScope::Library {
            let has_open_book = state.borrow().open_file().is_some();
            self.snapshot = has_open_book.then(|| RuntimeSnapshot {
                view: current_runtime_view(state, viewport),
                inherit_pending: state.borrow().is_inherit_pending(),
            });
            apply_global_view_to_runtime(settings, state, viewport);
        }
    }

    pub fn scope(&self) -> Option<DialogScope> {
        self.scope
    }

    /// End the current session. Restore a Library-scope snapshot, then clear
    /// all session state. Calling this without an active session is a no-op.
    pub fn end(&mut self, state: &Rc<RefCell<ViewerState>>, viewport: &Rc<RefCell<ViewportState>>) {
        if let Some(snapshot) = self.snapshot.take() {
            state
                .borrow_mut()
                .apply_resolved_view(snapshot.view, &mut viewport.borrow_mut());
            // AFTER apply_resolved_view: its set_* calls clear the flag (#415 order).
            if snapshot.inherit_pending {
                state.borrow_mut().mark_inherit_pending();
            } else {
                state.borrow_mut().clear_inherit_pending();
            }
        }
        self.scope = None;
        self.snapshot = None;
    }

    /// Reset-to-global: apply globals to the runtime, THEN
    /// `mark_inherit_pending`. Owns the #415 order.
    pub fn reset_to_global(
        state: &Rc<RefCell<ViewerState>>,
        viewport: &Rc<RefCell<ViewportState>>,
        settings: &Rc<RefCell<Settings>>,
    ) {
        apply_global_view_to_runtime(settings, state, viewport);
        state.borrow_mut().mark_inherit_pending();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gashuu_core::{CoverMode, FitMode, ReadingDirection, SpreadMode};

    fn global_settings() -> Rc<RefCell<Settings>> {
        Rc::new(RefCell::new(Settings {
            reading_direction: ReadingDirection::Rtl,
            spread_mode: SpreadMode::Double,
            cover_mode: CoverMode::Paired,
            fit_mode: FitMode::Actual,
            ..Settings::default()
        }))
    }

    fn global_view(settings: &Rc<RefCell<Settings>>) -> ResolvedView {
        let settings = settings.borrow();
        ResolvedView {
            reading_direction: settings.reading_direction,
            spread_mode: settings.spread_mode,
            cover_mode: settings.cover_mode,
            fit_mode: settings.fit_mode,
        }
    }

    fn book_view() -> ResolvedView {
        ResolvedView {
            reading_direction: ReadingDirection::Ltr,
            spread_mode: SpreadMode::Single,
            cover_mode: CoverMode::Standalone,
            fit_mode: FitMode::Whole,
        }
    }

    fn alternate_book_view() -> ResolvedView {
        ResolvedView {
            reading_direction: ReadingDirection::Ltr,
            spread_mode: SpreadMode::Auto,
            cover_mode: CoverMode::Paired,
            fit_mode: FitMode::Width,
        }
    }

    fn apply_runtime_view(
        state: &Rc<RefCell<ViewerState>>,
        viewport: &Rc<RefCell<ViewportState>>,
        view: ResolvedView,
    ) {
        state
            .borrow_mut()
            .apply_resolved_view(view, &mut viewport.borrow_mut());
    }

    fn open_book(state: &Rc<RefCell<ViewerState>>) -> tempfile::TempDir {
        let dir = tempfile::tempdir().expect("create book directory");
        state
            .borrow_mut()
            .open_path(dir.path())
            .expect("open book directory");
        dir
    }

    #[test]
    fn open_with_book_then_close_restores_books_runtime() {
        let settings = global_settings();
        let state = Rc::new(RefCell::new(ViewerState::new()));
        let viewport = Rc::new(RefCell::new(ViewportState::from_settings(
            &settings.borrow(),
        )));
        let _book = open_book(&state);
        apply_runtime_view(&state, &viewport, book_view());
        let mut session = DialogSession::new();

        session.open(DialogScope::Library, &state, &viewport, &settings);
        assert_eq!(
            current_runtime_view(&state, &viewport),
            global_view(&settings)
        );

        session.end(&state, &viewport);
        assert_eq!(current_runtime_view(&state, &viewport), book_view());
    }

    #[test]
    fn open_without_book_then_close_is_no_op() {
        let settings = global_settings();
        let state = Rc::new(RefCell::new(ViewerState::new()));
        let viewport = Rc::new(RefCell::new(ViewportState::from_settings(
            &settings.borrow(),
        )));
        apply_runtime_view(&state, &viewport, book_view());
        let mut session = DialogSession::new();

        session.open(DialogScope::Library, &state, &viewport, &settings);
        assert_eq!(
            current_runtime_view(&state, &viewport),
            global_view(&settings)
        );

        session.end(&state, &viewport);
        assert_eq!(
            current_runtime_view(&state, &viewport),
            global_view(&settings)
        );
    }

    #[test]
    fn second_open_before_close_replaces_snapshot() {
        let settings = global_settings();
        let state = Rc::new(RefCell::new(ViewerState::new()));
        let viewport = Rc::new(RefCell::new(ViewportState::from_settings(
            &settings.borrow(),
        )));
        let _book = open_book(&state);
        apply_runtime_view(&state, &viewport, book_view());
        let mut session = DialogSession::new();

        session.open(DialogScope::Library, &state, &viewport, &settings);
        apply_runtime_view(&state, &viewport, alternate_book_view());
        session.open(DialogScope::Library, &state, &viewport, &settings);
        assert_eq!(
            current_runtime_view(&state, &viewport),
            global_view(&settings)
        );

        session.end(&state, &viewport);
        assert_eq!(
            current_runtime_view(&state, &viewport),
            alternate_book_view()
        );
    }

    #[test]
    fn reset_to_global_applies_globals_then_marks_inherit_pending() {
        let settings = global_settings();
        let state = Rc::new(RefCell::new(ViewerState::new()));
        let viewport = Rc::new(RefCell::new(ViewportState::from_settings(
            &settings.borrow(),
        )));
        apply_runtime_view(&state, &viewport, book_view());

        DialogSession::reset_to_global(&state, &viewport, &settings);

        assert_eq!(
            current_runtime_view(&state, &viewport),
            global_view(&settings)
        );
        assert!(state.borrow().is_inherit_pending());
    }

    #[test]
    fn library_dialog_global_edit_preserves_inherit_pending() {
        let settings = global_settings();
        let state = Rc::new(RefCell::new(ViewerState::new()));
        let viewport = Rc::new(RefCell::new(ViewportState::from_settings(
            &settings.borrow(),
        )));
        let _book = open_book(&state);
        apply_runtime_view(&state, &viewport, book_view());
        DialogSession::reset_to_global(&state, &viewport, &settings);
        assert!(state.borrow().is_inherit_pending());
        let mut session = DialogSession::new();

        session.open(DialogScope::Library, &state, &viewport, &settings);
        state.borrow_mut().set_spread_mode(SpreadMode::Single);
        session.end(&state, &viewport);

        assert_eq!(
            current_runtime_view(&state, &viewport),
            global_view(&settings)
        );
        assert!(state.borrow().is_inherit_pending());
    }

    #[test]
    fn library_dialog_restores_flag_verbatim_when_not_pending() {
        let settings = global_settings();
        let state = Rc::new(RefCell::new(ViewerState::new()));
        let viewport = Rc::new(RefCell::new(ViewportState::from_settings(
            &settings.borrow(),
        )));
        let _book = open_book(&state);
        apply_runtime_view(&state, &viewport, book_view());
        assert!(!state.borrow().is_inherit_pending());
        let mut session = DialogSession::new();

        session.open(DialogScope::Library, &state, &viewport, &settings);
        state.borrow_mut().set_spread_mode(SpreadMode::Single);
        session.end(&state, &viewport);

        assert!(!state.borrow().is_inherit_pending());
    }

    #[test]
    fn library_session_ended_by_exit_restores_the_books_runtime() {
        let settings = global_settings();
        let state = Rc::new(RefCell::new(ViewerState::new()));
        let viewport = Rc::new(RefCell::new(ViewportState::from_settings(
            &settings.borrow(),
        )));
        let _book = open_book(&state);
        apply_runtime_view(&state, &viewport, book_view());
        let mut session = DialogSession::new();

        session.open(DialogScope::Library, &state, &viewport, &settings);
        assert_eq!(
            current_runtime_view(&state, &viewport),
            global_view(&settings)
        );

        session.end(&state, &viewport);
        assert_eq!(current_runtime_view(&state, &viewport), book_view());
        assert_eq!(session.scope(), None);
    }

    #[test]
    fn library_session_ended_twice_is_a_no_op() {
        let settings = global_settings();
        let state = Rc::new(RefCell::new(ViewerState::new()));
        let viewport = Rc::new(RefCell::new(ViewportState::from_settings(
            &settings.borrow(),
        )));
        let _book = open_book(&state);
        apply_runtime_view(&state, &viewport, book_view());
        let mut session = DialogSession::new();

        session.open(DialogScope::Library, &state, &viewport, &settings);
        session.end(&state, &viewport);
        apply_runtime_view(&state, &viewport, alternate_book_view());
        session.end(&state, &viewport);

        assert_eq!(
            current_runtime_view(&state, &viewport),
            alternate_book_view()
        );
    }

    #[test]
    fn viewer_session_end_clears_without_restoring() {
        let settings = global_settings();
        let state = Rc::new(RefCell::new(ViewerState::new()));
        let viewport = Rc::new(RefCell::new(ViewportState::from_settings(
            &settings.borrow(),
        )));
        apply_runtime_view(&state, &viewport, book_view());
        let mut session = DialogSession::new();

        session.open(DialogScope::Viewer, &state, &viewport, &settings);
        assert_eq!(current_runtime_view(&state, &viewport), book_view());

        session.end(&state, &viewport);
        assert_eq!(current_runtime_view(&state, &viewport), book_view());
        assert_eq!(session.scope(), None);
    }

    #[test]
    fn session_opened_on_library_and_ended_from_viewer_restores_the_book() {
        let settings = global_settings();
        let state = Rc::new(RefCell::new(ViewerState::new()));
        let viewport = Rc::new(RefCell::new(ViewportState::from_settings(
            &settings.borrow(),
        )));
        let _book = open_book(&state);
        apply_runtime_view(&state, &viewport, book_view());
        let mut session = DialogSession::new();

        session.open(DialogScope::Library, &state, &viewport, &settings);
        assert_eq!(
            current_runtime_view(&state, &viewport),
            global_view(&settings)
        );

        session.end(&state, &viewport);
        assert_eq!(current_runtime_view(&state, &viewport), book_view());
        assert_eq!(session.scope(), None);
        assert!(session.snapshot.is_none());
    }
}
