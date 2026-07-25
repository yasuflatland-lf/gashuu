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

#[cfg(test)]
mod tests {
    use super::{is_action_allowed, UpdateDialogAction};

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
}
