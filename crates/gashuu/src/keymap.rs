//! Maps UI key tokens to navigation actions. The `.slint` FocusScope decodes the
//! physical key against Slint's `Key.*` constants and forwards a stable token
//! ("left"/"right"/"space"/"backspace"); this keeps the mapping pure and testable.
//!
//! PR1 is LTR: RightArrow/Space advance, LeftArrow/Backspace go back. RTL is PR4.

/// A page-turn direction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NavAction {
    Next,
    Prev,
}

/// Map a UI key token to a navigation action, or `None` for unhandled keys.
pub fn map_key(token: &str) -> Option<NavAction> {
    match token {
        "right" | "space" => Some(NavAction::Next),
        "left" | "backspace" => Some(NavAction::Prev),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn forward_keys_map_to_next() {
        assert_eq!(map_key("right"), Some(NavAction::Next));
        assert_eq!(map_key("space"), Some(NavAction::Next));
    }

    #[test]
    fn backward_keys_map_to_prev() {
        assert_eq!(map_key("left"), Some(NavAction::Prev));
        assert_eq!(map_key("backspace"), Some(NavAction::Prev));
    }

    #[test]
    fn unknown_keys_map_to_none() {
        assert_eq!(map_key("enter"), None);
        assert_eq!(map_key(""), None);
    }
}
