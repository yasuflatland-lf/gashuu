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
    fn update_dialog_action_gating_table() {
        use UpdateDialogAction::{Accept, Later, Notes, Skip};

        for (action, in_progress, expected) in [
            (Accept, false, true),
            (Accept, true, false),
            (Later, false, true),
            (Later, true, false),
            (Skip, false, true),
            (Skip, true, false),
            (Notes, false, true),
            (Notes, true, true),
        ] {
            assert_eq!(
                is_action_allowed(action, in_progress),
                expected,
                "{action:?}, in_progress={in_progress}",
            );
        }
    }

    #[test]
    fn update_dialog_focus_target_table() {
        use FocusTarget::{Carousel, ConfirmDialog, Pages, SettingsDialog, ShortcutsOverlay};

        for (settings, shortcuts, confirm_delete, screen, expected) in [
            (false, false, false, 0, Carousel),
            (false, false, false, 1, Pages),
            (false, false, false, 7, Pages),
            (false, false, false, -1, Pages),
            (false, false, true, 0, ConfirmDialog),
            (false, false, true, 1, ConfirmDialog),
            (false, false, true, 7, ConfirmDialog),
            (false, false, true, -1, ConfirmDialog),
            (false, true, false, 0, ShortcutsOverlay),
            (false, true, false, 1, ShortcutsOverlay),
            (false, true, false, 7, ShortcutsOverlay),
            (false, true, false, -1, ShortcutsOverlay),
            (false, true, true, 0, ConfirmDialog),
            (false, true, true, 1, ConfirmDialog),
            (false, true, true, 7, ConfirmDialog),
            (false, true, true, -1, ConfirmDialog),
            (true, false, false, 0, SettingsDialog),
            (true, false, false, 1, SettingsDialog),
            (true, false, false, 7, SettingsDialog),
            (true, false, false, -1, SettingsDialog),
            (true, false, true, 0, ConfirmDialog),
            (true, false, true, 1, ConfirmDialog),
            (true, false, true, 7, ConfirmDialog),
            (true, false, true, -1, ConfirmDialog),
            (true, true, false, 0, ShortcutsOverlay),
            (true, true, false, 1, ShortcutsOverlay),
            (true, true, false, 7, ShortcutsOverlay),
            (true, true, false, -1, ShortcutsOverlay),
            (true, true, true, 0, ConfirmDialog),
            (true, true, true, 1, ConfirmDialog),
            (true, true, true, 7, ConfirmDialog),
            (true, true, true, -1, ConfirmDialog),
        ] {
            let stack = ModalStack {
                settings,
                shortcuts,
                confirm_delete,
            };
            assert_eq!(
                focus_target_after_dialog(stack, screen),
                expected,
                "{stack:?}, screen={screen}",
            );
        }
    }
}
