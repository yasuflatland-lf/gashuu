//! Callback wiring for the Slint window, grouped by feature (#151).
//! Each `wire_*` fn takes exactly the handles its closures clone — the
//! per-closure `Rc::clone` list is that handler's dependency list.
//! Each wire_* fn rebinds its `&Rc` params to owned locals (`let x = Rc::clone(x);`)
//! so the per-closure `Rc::clone(&x)` prelude lines stay byte-identical to their
//! pre-extraction form in `fn main` (and clippy::needless_borrow stays quiet).
use crate::ViewerWindow;
use gashuu_core::CoreError;

pub(crate) mod drag_drop;
pub(crate) mod library;
pub(crate) mod settings;
pub(crate) mod update;
pub(crate) mod viewer;

// Re-export each wire_* fn (plus start_update_check) at the handlers:: level so main.rs needs no sub-module path.
pub(crate) use drag_drop::wire_drag_drop_handlers;
pub(crate) use library::{wire_carousel_handlers, wire_open_handlers, wire_selection_handlers};
pub(crate) use settings::{wire_settings_handlers, wire_view_mode_handlers};
pub(crate) use update::{start_update_check, wire_update_handlers};
pub(crate) use viewer::{wire_nav_handlers, wire_viewer_input_handlers, wire_viewport_handlers};

/// Log a failed persistence save and surface it on the status line — the single
/// home of the direct log+status save-failure shape shared by callback handlers.
/// Aggregated notices and headless/exit paths keep their own handling.
fn report_save_error(
    ui: &ViewerWindow,
    loader: &i18n_embed::fluent::FluentLanguageLoader,
    e: &CoreError,
    context: &'static str,
) {
    tracing::error!(error = %e, "{context}");
    ui.set_status_text(crate::i18n::dynamic::could_not_save_settings(loader, e).into());
}
