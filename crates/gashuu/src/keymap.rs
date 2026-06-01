//! Maps UI key tokens to UI commands. The `.slint` FocusScope decodes the
//! physical key against Slint's `Key.*` constants and forwards a stable token
//! string; this keeps the mapping pure and testable.
//!
//! ## Direction-aware arrows
//! Arrow keys depend on the active `ReadingDirection`:
//! - **LTR** (left-to-right): RightArrow → Next, LeftArrow → Prev.
//! - **RTL** (right-to-left): LeftArrow → Next, RightArrow → Prev.
//!
//! ## Reading-order keys
//! Space and Backspace are fixed in reading order regardless of direction:
//! Space → Next, Backspace → Prev.
//!
//! ## Mode-toggle keys
//! `d` emits `ToggleSpread` (the configured spread mode then cycles single →
//! double → auto in `ViewerState::toggle_spread`), `r` toggles reading direction
//! (Ltr <-> Rtl), `c` toggles cover page.
//!
//! ## Chrome reveal (PR-S)
//! The page scrubber's auto-hiding chrome reveals on arrow / page-turn keys, but
//! that reveal is a UI side effect handled in `main.rs`'s `on_nav` handler — NOT
//! here. `map_key` stays a pure token -> `KeyCommand` function with no UI
//! awareness, so do not add a reveal command or side effect to this module.
//!
//! ## Screen-navigation keys
//! `Up` emits `GoToLibrary` (direction-independent like the zoom/fit keys); it
//! returns the app to the Library screen and is only acted on in the Viewer.

use gashuu_core::ReadingDirection;

/// A page-turn direction in reading order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NavAction {
    /// Advance to the next page (forward in reading order).
    Next,
    /// Return to the previous page (backward in reading order).
    Prev,
}

/// A decoded UI command: a page turn (in reading order), a mode toggle, or a
/// zoom/fit action.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyCommand {
    Turn(NavAction),
    ToggleSpread,
    ToggleReadingDirection,
    ToggleCover,
    ToggleThumbnails,
    ZoomIn,
    ZoomOut,
    /// Reset zoom to 1.0 and center the pan position.
    ResetView,
    /// Set fit mode to Actual (1:1 pixel mapping).
    FitActual,
    /// Cycle fit mode: Whole -> Width -> Actual -> Whole.
    CycleFit,
    /// Return to the Library screen. Only meaningful in the Viewer screen
    /// (the screen gate lives in `main.rs`); direction-independent like the
    /// zoom/fit keys (Up has no reading-order meaning).
    GoToLibrary,
}

/// Map a UI key token to a command. Arrow keys depend on `reading_direction`
/// (RTL: Left advances/forward, Right goes back; LTR: the reverse). Space/Backspace
/// are reading-order (Next/Prev) regardless of direction. d/r/c toggle modes.
pub fn map_key(token: &str, dir: ReadingDirection) -> Option<KeyCommand> {
    match token {
        "right" => match dir {
            ReadingDirection::Ltr => Some(KeyCommand::Turn(NavAction::Next)),
            ReadingDirection::Rtl => Some(KeyCommand::Turn(NavAction::Prev)),
        },
        "left" => match dir {
            ReadingDirection::Ltr => Some(KeyCommand::Turn(NavAction::Prev)),
            ReadingDirection::Rtl => Some(KeyCommand::Turn(NavAction::Next)),
        },
        "space" => Some(KeyCommand::Turn(NavAction::Next)),
        "backspace" => Some(KeyCommand::Turn(NavAction::Prev)),
        "d" => Some(KeyCommand::ToggleSpread),
        "r" => Some(KeyCommand::ToggleReadingDirection),
        "c" => Some(KeyCommand::ToggleCover),
        "t" => Some(KeyCommand::ToggleThumbnails),
        "+" | "=" => Some(KeyCommand::ZoomIn),
        "-" => Some(KeyCommand::ZoomOut),
        "0" => Some(KeyCommand::ResetView),
        "1" => Some(KeyCommand::FitActual),
        "f" => Some(KeyCommand::CycleFit),
        "up" => Some(KeyCommand::GoToLibrary),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gashuu_core::ReadingDirection;

    // --- Arrow keys: direction-dependent ---

    #[test]
    fn right_arrow_ltr_maps_to_next() {
        assert_eq!(
            map_key("right", ReadingDirection::Ltr),
            Some(KeyCommand::Turn(NavAction::Next))
        );
    }

    #[test]
    fn left_arrow_ltr_maps_to_prev() {
        assert_eq!(
            map_key("left", ReadingDirection::Ltr),
            Some(KeyCommand::Turn(NavAction::Prev))
        );
    }

    #[test]
    fn right_arrow_rtl_maps_to_prev() {
        assert_eq!(
            map_key("right", ReadingDirection::Rtl),
            Some(KeyCommand::Turn(NavAction::Prev))
        );
    }

    #[test]
    fn left_arrow_rtl_maps_to_next() {
        assert_eq!(
            map_key("left", ReadingDirection::Rtl),
            Some(KeyCommand::Turn(NavAction::Next))
        );
    }

    // --- Space/Backspace: reading-order regardless of direction ---

    #[test]
    fn space_maps_to_next_ltr() {
        assert_eq!(
            map_key("space", ReadingDirection::Ltr),
            Some(KeyCommand::Turn(NavAction::Next))
        );
    }

    #[test]
    fn space_maps_to_next_rtl() {
        assert_eq!(
            map_key("space", ReadingDirection::Rtl),
            Some(KeyCommand::Turn(NavAction::Next))
        );
    }

    #[test]
    fn backspace_maps_to_prev_ltr() {
        assert_eq!(
            map_key("backspace", ReadingDirection::Ltr),
            Some(KeyCommand::Turn(NavAction::Prev))
        );
    }

    #[test]
    fn backspace_maps_to_prev_rtl() {
        assert_eq!(
            map_key("backspace", ReadingDirection::Rtl),
            Some(KeyCommand::Turn(NavAction::Prev))
        );
    }

    // --- Mode-toggle keys ---

    #[test]
    fn d_maps_to_toggle_spread() {
        assert_eq!(
            map_key("d", ReadingDirection::Ltr),
            Some(KeyCommand::ToggleSpread)
        );
        assert_eq!(
            map_key("d", ReadingDirection::Rtl),
            Some(KeyCommand::ToggleSpread)
        );
    }

    #[test]
    fn r_maps_to_toggle_reading_direction() {
        assert_eq!(
            map_key("r", ReadingDirection::Ltr),
            Some(KeyCommand::ToggleReadingDirection)
        );
        assert_eq!(
            map_key("r", ReadingDirection::Rtl),
            Some(KeyCommand::ToggleReadingDirection)
        );
    }

    #[test]
    fn c_maps_to_toggle_cover() {
        assert_eq!(
            map_key("c", ReadingDirection::Ltr),
            Some(KeyCommand::ToggleCover)
        );
        assert_eq!(
            map_key("c", ReadingDirection::Rtl),
            Some(KeyCommand::ToggleCover)
        );
    }

    #[test]
    fn t_maps_to_toggle_thumbnails() {
        assert_eq!(
            map_key("t", ReadingDirection::Ltr),
            Some(KeyCommand::ToggleThumbnails)
        );
        assert_eq!(
            map_key("t", ReadingDirection::Rtl),
            Some(KeyCommand::ToggleThumbnails)
        );
    }

    // --- Zoom / fit keys: direction-independent ---

    #[test]
    fn plus_maps_to_zoom_in() {
        assert_eq!(
            map_key("+", ReadingDirection::Ltr),
            Some(KeyCommand::ZoomIn)
        );
        assert_eq!(
            map_key("+", ReadingDirection::Rtl),
            Some(KeyCommand::ZoomIn)
        );
    }

    #[test]
    fn equals_maps_to_zoom_in() {
        assert_eq!(
            map_key("=", ReadingDirection::Ltr),
            Some(KeyCommand::ZoomIn)
        );
        assert_eq!(
            map_key("=", ReadingDirection::Rtl),
            Some(KeyCommand::ZoomIn)
        );
    }

    #[test]
    fn minus_maps_to_zoom_out() {
        assert_eq!(
            map_key("-", ReadingDirection::Ltr),
            Some(KeyCommand::ZoomOut)
        );
        assert_eq!(
            map_key("-", ReadingDirection::Rtl),
            Some(KeyCommand::ZoomOut)
        );
    }

    #[test]
    fn zero_maps_to_reset_view() {
        assert_eq!(
            map_key("0", ReadingDirection::Ltr),
            Some(KeyCommand::ResetView)
        );
        assert_eq!(
            map_key("0", ReadingDirection::Rtl),
            Some(KeyCommand::ResetView)
        );
    }

    #[test]
    fn one_maps_to_fit_actual() {
        assert_eq!(
            map_key("1", ReadingDirection::Ltr),
            Some(KeyCommand::FitActual)
        );
        assert_eq!(
            map_key("1", ReadingDirection::Rtl),
            Some(KeyCommand::FitActual)
        );
    }

    #[test]
    fn f_maps_to_cycle_fit() {
        assert_eq!(
            map_key("f", ReadingDirection::Ltr),
            Some(KeyCommand::CycleFit)
        );
        assert_eq!(
            map_key("f", ReadingDirection::Rtl),
            Some(KeyCommand::CycleFit)
        );
    }

    // --- Screen navigation keys: direction-independent ---

    #[test]
    fn up_maps_to_go_to_library() {
        assert_eq!(
            map_key("up", ReadingDirection::Ltr),
            Some(KeyCommand::GoToLibrary)
        );
        assert_eq!(
            map_key("up", ReadingDirection::Rtl),
            Some(KeyCommand::GoToLibrary)
        );
    }

    // --- Unknown tokens ---

    #[test]
    fn unknown_keys_map_to_none() {
        assert_eq!(map_key("enter", ReadingDirection::Ltr), None);
        assert_eq!(map_key("enter", ReadingDirection::Rtl), None);
        assert_eq!(map_key("", ReadingDirection::Ltr), None);
        assert_eq!(map_key("", ReadingDirection::Rtl), None);
    }
}
