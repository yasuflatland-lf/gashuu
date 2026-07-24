use crate::view_sync::{apply_global_view_to_runtime, current_runtime_view};
use crate::{viewer_state::ViewerState, viewport::ViewportState};
use gashuu_core::{ResolvedView, Settings};
use std::cell::RefCell;
use std::rc::Rc;

/// The settings-dialog session over the CURRENT screen: owns the open book's
/// pre-dialog runtime snapshot (#414) and the reset-to-global ordering (#415).
pub(crate) struct DialogSession {
    snapshot: Option<ResolvedView>,
}

impl DialogSession {
    pub fn new() -> Self {
        Self { snapshot: None }
    }

    /// Library-screen dialog open: snapshot the open book's runtime (`None`
    /// when no book is open), THEN seed the runtime with globals. Owns the #414
    /// order.
    pub fn open_on_library(
        &mut self,
        state: &Rc<RefCell<ViewerState>>,
        viewport: &Rc<RefCell<ViewportState>>,
        settings: &Rc<RefCell<Settings>>,
    ) {
        let has_open_book = state.borrow().open_file().is_some();
        self.snapshot = has_open_book.then(|| current_runtime_view(state, viewport));
        apply_global_view_to_runtime(settings, state, viewport);
    }

    /// Library-screen dialog close: restore the snapshot into the runtime
    /// (no-op when `None`). Uses the folded `apply_resolved_view` (#449).
    pub fn close_on_library(
        &mut self,
        state: &Rc<RefCell<ViewerState>>,
        viewport: &Rc<RefCell<ViewportState>>,
    ) {
        if let Some(snapshot) = self.snapshot.take() {
            state
                .borrow_mut()
                .apply_resolved_view(snapshot, &mut viewport.borrow_mut());
        }
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

        session.open_on_library(&state, &viewport, &settings);
        assert_eq!(
            current_runtime_view(&state, &viewport),
            global_view(&settings)
        );

        session.close_on_library(&state, &viewport);
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

        session.open_on_library(&state, &viewport, &settings);
        assert_eq!(
            current_runtime_view(&state, &viewport),
            global_view(&settings)
        );

        session.close_on_library(&state, &viewport);
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

        session.open_on_library(&state, &viewport, &settings);
        apply_runtime_view(&state, &viewport, alternate_book_view());
        session.open_on_library(&state, &viewport, &settings);
        assert_eq!(
            current_runtime_view(&state, &viewport),
            global_view(&settings)
        );

        session.close_on_library(&state, &viewport);
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
}
