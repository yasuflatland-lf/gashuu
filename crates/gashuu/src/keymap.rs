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
//! `d` toggles spread mode, `r` cycles reading direction, `c` toggles cover page.

use gashuu_core::ReadingDirection;

/// A page-turn direction in reading order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NavAction {
    /// Advance to the next page (forward in reading order).
    Next,
    /// Return to the previous page (backward in reading order).
    Prev,
}

/// A decoded UI command: a page turn (in reading order) or a mode toggle.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyCommand {
    Turn(NavAction),
    ToggleSpread,
    ToggleReadingDirection,
    ToggleCover,
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

    // --- Unknown tokens ---

    #[test]
    fn unknown_keys_map_to_none() {
        assert_eq!(map_key("enter", ReadingDirection::Ltr), None);
        assert_eq!(map_key("enter", ReadingDirection::Rtl), None);
        assert_eq!(map_key("", ReadingDirection::Ltr), None);
        assert_eq!(map_key("", ReadingDirection::Rtl), None);
    }
}
