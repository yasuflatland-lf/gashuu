use crate::view_sync::{apply_global_view_to_runtime, current_runtime_view};
use crate::{viewer_state::ViewerState, viewport::ViewportState};
use gashuu_core::{ResolvedView, Settings};
use std::cell::RefCell;
use std::rc::Rc;

/// The open book's pre-dialog runtime, captured at library-screen dialog open.
struct RuntimeSnapshot {
    view: ResolvedView,
    pending_inherit: Option<ResolvedView>,
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
    pending_inherit: Option<ResolvedView>,
}

impl DialogSession {
    pub fn new() -> Self {
        Self {
            scope: None,
            snapshot: None,
            pending_inherit: None,
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
                pending_inherit: self.pending_inherit,
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
            self.pending_inherit = snapshot.pending_inherit;
        }
        self.scope = None;
        self.snapshot = None;
    }

    /// Whether a reset is pending and the live runtime still matches the view
    /// installed by that reset.
    pub fn inherit_pending(&self, current: ResolvedView) -> bool {
        self.pending_inherit == Some(current)
    }

    /// Discard any pending reset intent for the current book.
    pub fn clear_pending_inherit(&mut self) {
        self.pending_inherit = None;
    }

    /// Reset-to-global: apply globals to the runtime, THEN record the installed
    /// view as pending inherit. Owns the #415 order.
    pub fn reset_to_global(
        &mut self,
        state: &Rc<RefCell<ViewerState>>,
        viewport: &Rc<RefCell<ViewportState>>,
        settings: &Rc<RefCell<Settings>>,
    ) {
        apply_global_view_to_runtime(settings, state, viewport);
        self.pending_inherit = Some(current_runtime_view(state, viewport));
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
    fn reset_then_no_change_is_inherit_pending() {
        let settings = global_settings();
        let state = Rc::new(RefCell::new(ViewerState::new()));
        let viewport = Rc::new(RefCell::new(ViewportState::from_settings(
            &settings.borrow(),
        )));
        apply_runtime_view(&state, &viewport, book_view());
        let mut session = DialogSession::new();

        session.reset_to_global(&state, &viewport, &settings);
        let current = current_runtime_view(&state, &viewport);

        assert!(session.inherit_pending(current));
    }

    #[test]
    fn reset_then_any_viewer_mode_change_clears_it() {
        type Mutation = fn(&mut ViewerState);
        let cases: [(&str, Mutation); 6] = [
            ("set_reading_direction", |state| {
                state.set_reading_direction(ReadingDirection::Ltr);
            }),
            ("set_spread_mode", |state| {
                state.set_spread_mode(SpreadMode::Single);
            }),
            ("set_cover_mode", |state| {
                state.set_cover_mode(CoverMode::Standalone);
            }),
            ("toggle_spread", |state| {
                state.toggle_spread();
            }),
            ("toggle_cover", |state| {
                state.toggle_cover();
            }),
            ("toggle_reading_direction", |state| {
                state.toggle_reading_direction();
            }),
        ];
        let mut failures = Vec::new();

        for (name, mutate) in cases {
            let settings = global_settings();
            let state = Rc::new(RefCell::new(ViewerState::new()));
            let viewport = Rc::new(RefCell::new(ViewportState::from_settings(
                &settings.borrow(),
            )));
            apply_runtime_view(&state, &viewport, book_view());
            let mut session = DialogSession::new();
            session.reset_to_global(&state, &viewport, &settings);

            mutate(&mut state.borrow_mut());
            let current = current_runtime_view(&state, &viewport);
            if session.inherit_pending(current) {
                failures.push(name.to_owned());
            }
        }

        assert!(failures.is_empty(), "{failures:#?}");
    }

    #[test]
    fn reset_then_fit_change_clears_it() {
        let settings = global_settings();
        let state = Rc::new(RefCell::new(ViewerState::new()));
        let viewport = Rc::new(RefCell::new(ViewportState::from_settings(
            &settings.borrow(),
        )));
        apply_runtime_view(&state, &viewport, book_view());
        let mut session = DialogSession::new();
        session.reset_to_global(&state, &viewport, &settings);

        viewport.borrow_mut().set_fit(FitMode::Whole);
        let current = current_runtime_view(&state, &viewport);

        assert!(!session.inherit_pending(current));
    }

    #[test]
    fn reset_then_change_and_change_back_is_pending_again() {
        let settings = global_settings();
        let state = Rc::new(RefCell::new(ViewerState::new()));
        let viewport = Rc::new(RefCell::new(ViewportState::from_settings(
            &settings.borrow(),
        )));
        apply_runtime_view(&state, &viewport, book_view());
        let mut session = DialogSession::new();
        session.reset_to_global(&state, &viewport, &settings);

        state.borrow_mut().set_spread_mode(SpreadMode::Single);
        state.borrow_mut().set_spread_mode(SpreadMode::Double);
        let current = current_runtime_view(&state, &viewport);

        assert!(session.inherit_pending(current));
    }

    #[test]
    fn snapshot_restore_preserves_pending_intent() {
        let settings = global_settings();
        let state = Rc::new(RefCell::new(ViewerState::new()));
        let viewport = Rc::new(RefCell::new(ViewportState::from_settings(
            &settings.borrow(),
        )));
        let _book = open_book(&state);
        let mut session = DialogSession::new();
        session.reset_to_global(&state, &viewport, &settings);

        session.open(DialogScope::Library, &state, &viewport, &settings);
        state.borrow_mut().set_spread_mode(SpreadMode::Single);
        session.end(&state, &viewport);
        let current = current_runtime_view(&state, &viewport);

        assert!(session.inherit_pending(current));
    }

    #[test]
    fn snapshot_restore_preserves_absence() {
        let settings = global_settings();
        let state = Rc::new(RefCell::new(ViewerState::new()));
        let viewport = Rc::new(RefCell::new(ViewportState::from_settings(
            &settings.borrow(),
        )));
        let _book = open_book(&state);
        apply_runtime_view(&state, &viewport, book_view());
        let mut session = DialogSession::new();

        session.open(DialogScope::Library, &state, &viewport, &settings);
        state.borrow_mut().set_spread_mode(SpreadMode::Single);
        session.end(&state, &viewport);
        let current = current_runtime_view(&state, &viewport);

        assert!(!session.inherit_pending(current));
    }

    #[test]
    fn clear_pending_inherit_is_idempotent() {
        let settings = global_settings();
        let state = Rc::new(RefCell::new(ViewerState::new()));
        let viewport = Rc::new(RefCell::new(ViewportState::from_settings(
            &settings.borrow(),
        )));
        let mut session = DialogSession::new();
        session.reset_to_global(&state, &viewport, &settings);

        session.clear_pending_inherit();
        session.clear_pending_inherit();
        let current = current_runtime_view(&state, &viewport);

        assert!(!session.inherit_pending(current));
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
