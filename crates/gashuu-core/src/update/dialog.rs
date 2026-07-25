//! Update-dialog action gating. Pure and deterministic — no `slint`, no I/O.
//! The UI crate owns the widgets; this module owns "may this action fire right
//! now", so the in-progress state machine is unit-testable headlessly.

/// The user decisions the update dialog offers. Dismissal gestures alias onto
/// them: Esc, Return and a backdrop click are all `Later`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UpdateDialogAction {
    /// "Update now" — starts the download/verify/install pipeline.
    Accept,
    /// "Later", Esc, Return, or a backdrop click — dismiss without acting.
    Later,
    /// "Skip this version" — dismiss and persist the skip.
    Skip,
    /// "Release notes" — opens a browser page; never dismisses the dialog.
    Notes,
}

/// Whether `action` may fire while a download/install is in flight.
///
/// BLOCK semantics: while `in_progress` the dialog is committed. `Accept` must
/// not re-enter the pipeline (two concurrent downloads / two racing binary
/// replacements), and `Later`/`Skip` must not dismiss a dialog whose background
/// job would then finish invisibly and force-restart the app. `Notes` stays
/// allowed: it neither dismisses the dialog nor touches the pipeline.
///
/// There is deliberately no cancellation: dismissal is BLOCKED, not aborted.
pub const fn is_action_allowed(action: UpdateDialogAction, in_progress: bool) -> bool {
    match action {
        UpdateDialogAction::Notes => true,
        UpdateDialogAction::Accept | UpdateDialogAction::Later | UpdateDialogAction::Skip => {
            !in_progress
        }
    }
}

/// The modals still standing underneath a just-dismissed update dialog.
/// `ViewerWindow`'s legal stacks are {Settings}, {Settings, Shortcuts},
/// {Confirm} and the empty stack (the update dialog is always topmost).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ModalStack {
    pub settings: bool,
    pub shortcuts: bool,
    pub confirm_delete: bool,
}

/// The surface that must own the keyboard after a modal above it is dismissed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FocusTarget {
    ConfirmDialog,
    ShortcutsOverlay,
    SettingsDialog,
    /// Library screen (screen 0).
    Carousel,
    /// Viewer screen (screen 1).
    Pages,
}

/// Topmost surviving surface, in `ViewerWindow`'s modal paint order
/// (ConfirmDialog > ShortcutsOverlay > SettingsDialog > screen).
///
/// INVARIANT (Z3 F-C4): whenever ANY modal is still open the result is a modal,
/// never a screen surface — the screen FocusScopes reject every key while
/// `any-modal-open`, so focusing one under an open modal is a keyboard deadlock.
pub const fn focus_target_after_dialog(stack: ModalStack, screen: i32) -> FocusTarget {
    if stack.confirm_delete {
        FocusTarget::ConfirmDialog
    } else if stack.shortcuts {
        FocusTarget::ShortcutsOverlay
    } else if stack.settings {
        FocusTarget::SettingsDialog
    } else if screen == 0 {
        FocusTarget::Carousel
    } else {
        FocusTarget::Pages
    }
}

#[cfg(test)]
mod tests {
    use super::{
        focus_target_after_dialog, is_action_allowed, FocusTarget, ModalStack, UpdateDialogAction,
    };

    #[test]
    fn all_actions_allowed_when_idle() {
        for action in [
            UpdateDialogAction::Accept,
            UpdateDialogAction::Later,
            UpdateDialogAction::Skip,
            UpdateDialogAction::Notes,
        ] {
            assert!(is_action_allowed(action, false));
        }
    }

    #[test]
    fn accept_blocked_while_in_progress() {
        assert!(!is_action_allowed(UpdateDialogAction::Accept, true));
    }

    #[test]
    fn later_and_skip_blocked_while_in_progress() {
        assert!(!is_action_allowed(UpdateDialogAction::Later, true));
        assert!(!is_action_allowed(UpdateDialogAction::Skip, true));
    }

    #[test]
    fn notes_allowed_while_in_progress() {
        assert!(is_action_allowed(UpdateDialogAction::Notes, true));
    }

    #[test]
    fn confirm_wins_over_every_other_surface() {
        let stack = ModalStack {
            settings: true,
            shortcuts: true,
            confirm_delete: true,
        };

        assert_eq!(
            focus_target_after_dialog(stack, 0),
            FocusTarget::ConfirmDialog
        );
        assert_eq!(
            focus_target_after_dialog(stack, 1),
            FocusTarget::ConfirmDialog
        );
    }

    #[test]
    fn shortcuts_wins_over_settings() {
        let stack = ModalStack {
            settings: true,
            shortcuts: true,
            confirm_delete: false,
        };

        assert_eq!(
            focus_target_after_dialog(stack, 0),
            FocusTarget::ShortcutsOverlay
        );
    }

    #[test]
    fn settings_alone_returns_settings() {
        let stack = ModalStack {
            settings: true,
            ..ModalStack::default()
        };

        assert_eq!(
            focus_target_after_dialog(stack, 0),
            FocusTarget::SettingsDialog
        );
        assert_eq!(
            focus_target_after_dialog(stack, 1),
            FocusTarget::SettingsDialog
        );
    }

    #[test]
    fn empty_stack_falls_back_to_the_screen_surface() {
        let stack = ModalStack::default();

        assert_eq!(focus_target_after_dialog(stack, 0), FocusTarget::Carousel);
        assert_eq!(focus_target_after_dialog(stack, 1), FocusTarget::Pages);
        assert_eq!(focus_target_after_dialog(stack, 7), FocusTarget::Pages);
    }

    #[test]
    fn never_focuses_a_screen_surface_while_a_modal_is_open() {
        for settings in [false, true] {
            for shortcuts in [false, true] {
                for confirm_delete in [false, true] {
                    let stack = ModalStack {
                        settings,
                        shortcuts,
                        confirm_delete,
                    };
                    if !settings && !shortcuts && !confirm_delete {
                        continue;
                    }

                    for screen in [0, 1] {
                        assert!(!matches!(
                            focus_target_after_dialog(stack, screen),
                            FocusTarget::Carousel | FocusTarget::Pages
                        ));
                    }
                }
            }
        }
    }
}
