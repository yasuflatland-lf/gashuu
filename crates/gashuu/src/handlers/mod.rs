//! Callback wiring for the Slint window, grouped by feature (#151).
//! Each `wire_*` fn takes exactly the handles its closures clone — the
//! per-closure `Rc::clone` list is that handler's dependency list.
//! Each wire_* fn rebinds its `&Rc` params to owned locals (`let x = Rc::clone(x);`)
//! so the per-closure `Rc::clone(&x)` prelude lines stay byte-identical to their
//! pre-extraction form in `fn main` (and clippy::needless_borrow stays quiet).
use crate::ViewerWindow;
use gashuu_core::CoreError;

#[cfg(not(target_arch = "wasm32"))]
pub(crate) mod drag_drop;
pub(crate) mod library;
pub(crate) mod settings;
#[cfg(not(target_arch = "wasm32"))]
pub(crate) mod update;
pub(crate) mod viewer;

// Re-export the wire_* fns and start_update_check at the handlers:: level.
// The drag_drop and update re-exports are desktop-only.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) use drag_drop::wire_drag_drop_handlers;
pub(crate) use library::{wire_carousel_handlers, wire_open_handlers, wire_selection_handlers};
pub(crate) use settings::{wire_settings_handlers, wire_view_mode_handlers};
#[cfg(not(target_arch = "wasm32"))]
pub(crate) use update::{start_update_check, wire_update_handlers};
pub(crate) use viewer::{wire_nav_handlers, wire_viewer_input_handlers, wire_viewport_handlers};

/// Which persisted store a failed save belongs to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SaveTarget {
    Library,
    Settings,
}

/// The localized status text for a failed save of `target`.
fn save_error_text(
    loader: &i18n_embed::fluent::FluentLanguageLoader,
    target: SaveTarget,
    e: &CoreError,
) -> String {
    match target {
        SaveTarget::Library => crate::i18n::dynamic::failed_save_library(loader, e),
        SaveTarget::Settings => crate::i18n::dynamic::could_not_save_settings(loader, e),
    }
}

/// Log a failed persistence save and surface it on the status line — the single
/// home of the direct log+status save-failure shape shared by callback handlers.
/// Aggregated notices and headless/exit paths keep their own handling.
fn report_save_error(
    ui: &ViewerWindow,
    loader: &i18n_embed::fluent::FluentLanguageLoader,
    target: SaveTarget,
    e: &CoreError,
    context: &'static str,
) {
    tracing::error!(error = %e, "{context}");
    ui.set_status_text(save_error_text(loader, target, e).into());
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::i18n::{dynamic, Localizer};
    use gashuu_core::Language;

    #[test]
    fn library_save_error_names_the_library() {
        let err = CoreError::from(std::io::Error::other("disk full"));
        for language in [Language::En, Language::Ja] {
            let localizer = Localizer::new(language);
            let loader = localizer.loader();
            let text = save_error_text(loader, SaveTarget::Library, &err);
            assert_eq!(text, dynamic::failed_save_library(loader, &err));
            assert_ne!(text, dynamic::could_not_save_settings(loader, &err));
        }
    }

    #[test]
    fn settings_save_error_names_the_settings() {
        let err = CoreError::from(std::io::Error::other("disk full"));
        for language in [Language::En, Language::Ja] {
            let localizer = Localizer::new(language);
            let loader = localizer.loader();
            assert_eq!(
                save_error_text(loader, SaveTarget::Settings, &err),
                dynamic::could_not_save_settings(loader, &err)
            );
        }
    }

    #[test]
    fn save_error_text_embeds_the_error_detail() {
        let err = CoreError::from(std::io::Error::other("disk full"));
        for language in [Language::En, Language::Ja] {
            let localizer = Localizer::new(language);
            for target in [SaveTarget::Library, SaveTarget::Settings] {
                let text = save_error_text(localizer.loader(), target, &err);
                assert!(text.contains(&err.to_string()));
            }
        }
    }
}
