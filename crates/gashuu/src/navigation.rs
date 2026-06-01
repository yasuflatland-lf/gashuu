//! Top-level screen state machine: the app is always on exactly one of two
//! screens — the Library carousel (home) or the Viewer (page spread).
//!
//! `NavState` keeps the current `Screen` private and is mutated only via the
//! intent-named transitions `to_library`/`to_viewer` (mirroring the private
//! fields + intent-named methods convention used by `ViewportState`). The app
//! boots to `Library` (the carousel is home, even when empty — see the design
//! doc DESIGN.md, "Layout / Two screens"). Mapping the enum to/from the Slint
//! `screen` int property lives in `screen_to_index`/`index_to_screen`, which
//! stay in lock-step with the
//! `ViewerWindow.screen` contract (0 = Library, 1 = Viewer).

/// One of the two top-level screens.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Screen {
    /// The cover-flow library carousel (home screen, including when empty).
    Library,
    /// The manga page viewer.
    Viewer,
}

/// Owns the current screen. App boots to `Library`.
pub struct NavState {
    screen: Screen,
}

impl NavState {
    /// Construct the initial state. The app boots to the Library carousel.
    pub fn new() -> Self {
        Self {
            screen: Screen::Library,
        }
    }

    /// The current screen.
    pub fn screen(&self) -> Screen {
        self.screen
    }

    // `to_library`/`to_viewer` are intent-named state TRANSITIONS (they mutate),
    // not value conversions; the `to_` prefix is deliberate per this module's
    // docstring, so the `to_*`-takes-`&self` convention does not apply.
    /// Switch to the Library carousel (e.g. Up arrow from the Viewer).
    #[allow(clippy::wrong_self_convention)]
    pub fn to_library(&mut self) {
        self.screen = Screen::Library;
    }

    /// Switch to the Viewer (e.g. Return on a focused book, or Down to return
    /// to the currently-open book).
    #[allow(clippy::wrong_self_convention)]
    pub fn to_viewer(&mut self) {
        self.screen = Screen::Viewer;
    }
}

impl Default for NavState {
    fn default() -> Self {
        Self::new()
    }
}

/// Map a `Screen` to the `ViewerWindow.screen` int property. Exhaustive match
/// so a new `Screen` variant becomes a compile error here. The ordering is
/// authoritative and MUST match the `ViewerWindow.slint` contract:
///   Library = 0, Viewer = 1.
pub fn screen_to_index(s: Screen) -> i32 {
    match s {
        Screen::Library => 0,
        Screen::Viewer => 1,
    }
}

/// Map a raw `screen` int back to a `Screen`, defaulting any out-of-range value
/// to the FIRST variant (`Library`) — mirroring the `index_to_*` clamp policy in
/// `main.rs`. (Currently the int only flows Rust -> Slint, but the helper keeps
/// the round-trip symmetric and is unit-tested for the clamp.)
// Completes the screen<->index contract symmetry (mirrors main.rs's index_to_*
// pattern) and is unit-tested for the clamp; only the tests use it for now.
#[allow(dead_code)]
pub fn index_to_screen(i: i32) -> Screen {
    match i {
        1 => Screen::Viewer,
        _ => Screen::Library,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_starts_at_library() {
        assert_eq!(NavState::new().screen(), Screen::Library);
    }

    #[test]
    fn to_viewer_then_to_library_transitions() {
        let mut nav = NavState::new();
        nav.to_viewer();
        assert_eq!(nav.screen(), Screen::Viewer);
        nav.to_library();
        assert_eq!(nav.screen(), Screen::Library);
    }

    #[test]
    fn transitions_are_idempotent() {
        let mut nav = NavState::new();
        nav.to_library();
        assert_eq!(nav.screen(), Screen::Library);
        nav.to_viewer();
        nav.to_viewer();
        assert_eq!(nav.screen(), Screen::Viewer);
    }

    #[test]
    fn screen_index_round_trips() {
        for s in [Screen::Library, Screen::Viewer] {
            assert_eq!(index_to_screen(screen_to_index(s)), s);
        }
    }

    #[test]
    fn screen_to_index_matches_slint_contract() {
        assert_eq!(screen_to_index(Screen::Library), 0);
        assert_eq!(screen_to_index(Screen::Viewer), 1);
    }

    #[test]
    fn out_of_range_index_clamps_to_library() {
        for bad in [-1, 2, 99, i32::MIN, i32::MAX] {
            assert_eq!(index_to_screen(bad), Screen::Library);
        }
    }
}
