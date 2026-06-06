//! Callback wiring for the Slint window, grouped by feature (#151).
//! Each `wire_*` fn takes exactly the handles its closures clone — the
//! per-closure `Rc::clone` list is that handler's dependency list.
//! Each wire_* fn rebinds its `&Rc` params to owned locals (`let x = Rc::clone(x);`)
//! so the per-closure `Rc::clone(&x)` prelude lines stay byte-identical to their
//! pre-extraction form in `fn main` (and clippy::needless_borrow stays quiet).
pub(crate) mod library;
pub(crate) mod settings;
pub(crate) mod viewer;

// Re-export each wire_* fn at the handlers:: level so main.rs needs no sub-module path.
pub(crate) use library::{wire_carousel_handlers, wire_open_handlers, wire_selection_handlers};
pub(crate) use settings::{wire_settings_handlers, wire_view_mode_handlers};
pub(crate) use viewer::{wire_nav_handlers, wire_viewer_input_handlers, wire_viewport_handlers};
