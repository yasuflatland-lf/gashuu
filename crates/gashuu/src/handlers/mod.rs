//! Callback wiring for the Slint window, grouped by feature (#151).
//! Each `wire_*` fn takes exactly the handles its closures clone — the
//! per-closure `Rc::clone` list is that handler's dependency list.
pub(crate) mod library;

pub(crate) use library::{wire_carousel_handlers, wire_open_handlers, wire_selection_handlers};
