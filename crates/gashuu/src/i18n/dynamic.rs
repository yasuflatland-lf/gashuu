//! Fluent-backed dynamic message functions.
//!
//! These functions replace the former `messages.rs` exhaustive-match catalog
//! for all runtime-composed strings (status line, notices, error messages).
//! Each function takes a `&FluentLanguageLoader` borrowed from a [`super::Localizer`]
//! and returns a freshly formatted [`String`] in the active locale.
//!
//! The `format_status` and `format_notices` aggregators are gated behind the
//! types from Wave 1B (`crate::app`) and Wave 2 (`crate::viewer_state`); they
//! will not compile until those waves land.  Individual message functions that
//! depend only on `gashuu_core` types compile immediately.

use crate::app::{NoticesContent, SkippedDetail};
use crate::viewer_state::{StatusContent, StatusKind};
use gashuu_core::{ReadingDirection, SpreadMode};
use i18n_embed::fluent::FluentLanguageLoader;
use i18n_embed_fl::fl;
use std::fmt::Display;

// ---- Static no-arg messages -----------------------------------------------

/// Status line when no source has been opened yet.
pub(crate) fn no_folder_opened(loader: &FluentLanguageLoader) -> String {
    fl!(loader, "viewer-no-folder")
}

/// Status line when the opened source contains no displayable images.
pub(crate) fn no_images(loader: &FluentLanguageLoader) -> String {
    fl!(loader, "viewer-no-images")
}

// ---- Enum-dispatch label functions ----------------------------------------

/// Compact spread-mode label for the status line's `[mode · direction]` tail.
pub(crate) fn spread_label(loader: &FluentLanguageLoader, mode: SpreadMode) -> String {
    match mode {
        SpreadMode::Single => fl!(loader, "viewer-spread-single"),
        SpreadMode::Double => fl!(loader, "viewer-spread-double"),
        SpreadMode::Auto => fl!(loader, "viewer-spread-auto"),
    }
}

/// Compact reading-direction label for the status line's `[mode · direction]` tail.
pub(crate) fn direction_label(loader: &FluentLanguageLoader, dir: ReadingDirection) -> String {
    match dir {
        ReadingDirection::Ltr => fl!(loader, "viewer-direction-ltr"),
        ReadingDirection::Rtl => fl!(loader, "viewer-direction-rtl"),
    }
}

// ---- Parameterized error / status messages --------------------------------

/// Status line for a failed open (the path did not open as a source).
///
/// Production composes failed opens from a pre-captured error string via
/// [`open_error_str`] (the `OpenOutcome::Error` payload), so this `&dyn Display`
/// flavor currently has only test callers; kept as the parallel to the other
/// `&dyn Display` message functions. `#[allow(dead_code)]` mirrors the test-only
/// accessor convention used elsewhere in the crate.
#[allow(dead_code)]
pub(crate) fn open_error(loader: &FluentLanguageLoader, e: &dyn Display) -> String {
    let error = e.to_string();
    fl!(loader, "viewer-open-error", error = error.as_str())
}

/// Status line for a failed open when the error is already a pre-formatted
/// string (e.g. from `OpenOutcome::Error`).
pub(crate) fn open_error_str(loader: &FluentLanguageLoader, e_str: &str) -> String {
    fl!(loader, "viewer-open-error", error = e_str)
}

/// Parenthesized marker for the trailing page of a spread that failed to decode.
/// `page` is 1-based.
pub(crate) fn page_unavailable(loader: &FluentLanguageLoader, page: usize) -> String {
    let page = page as i64;
    fl!(loader, "viewer-page-unavailable", page = page)
}

/// Status line when the leading page of the current spread failed to decode.
pub(crate) fn decode_error(loader: &FluentLanguageLoader, e: &dyn Display) -> String {
    let error = e.to_string();
    fl!(loader, "viewer-decode-error", error = error.as_str())
}

// ---- Notice messages -------------------------------------------------------

/// Notice when the open-path settings save (recents tracking) failed.
pub(crate) fn failed_save_settings(loader: &FluentLanguageLoader, e: &dyn Display) -> String {
    let error = e.to_string();
    fl!(
        loader,
        "notice-failed-save-settings",
        error = error.as_str()
    )
}

/// Notice when the open-path library save failed.
pub(crate) fn failed_save_library(loader: &FluentLanguageLoader, e: &dyn Display) -> String {
    let error = e.to_string();
    fl!(loader, "notice-failed-save-library", error = error.as_str())
}

/// Status line when saving settings from the dialog (or an override reset) failed.
pub(crate) fn could_not_save_settings(loader: &FluentLanguageLoader, e: &dyn Display) -> String {
    let error = e.to_string();
    fl!(
        loader,
        "notice-could-not-save-settings",
        error = error.as_str()
    )
}

/// Boot notice when persisted state failed to load.
/// `what` is the technical failure list (e.g. `"settings (...)"`) and stays
/// untranslated: when the settings file itself is corrupt the language
/// preference is unknown anyway.
pub(crate) fn load_failed(loader: &FluentLanguageLoader, what: &str) -> String {
    fl!(loader, "notice-load-failed", what = what)
}

/// Notice when every picked path was already in the library.
pub(crate) fn already_in_library(loader: &FluentLanguageLoader) -> String {
    fl!(loader, "notice-already-in-library")
}

/// Notice after a successful add. `n` is the number of books added.
pub(crate) fn added_books(loader: &FluentLanguageLoader, n: usize) -> String {
    let n = n as i64;
    fl!(loader, "notice-added-books", n = n)
}

/// Notice when books were added but the library save failed.
pub(crate) fn added_books_save_failed(
    loader: &FluentLanguageLoader,
    n: usize,
    e: &dyn Display,
) -> String {
    let n = n as i64;
    let error = e.to_string();
    fl!(
        loader,
        "notice-added-books-save-failed",
        n = n,
        error = error.as_str()
    )
}

// ---- Shortcuts help -------------------------------------------------------

/// Multi-line keyboard-shortcuts reference rendered read-only in the
/// ShortcutsOverlay.
pub(crate) fn shortcuts_help(loader: &FluentLanguageLoader) -> String {
    fl!(loader, "shortcuts-help")
}

// ---- Aggregator functions (require Wave 1B + Wave 2 types) ----------------

/// Format the viewer status line from a [`StatusContent`] snapshot.
pub(crate) fn format_status(loader: &FluentLanguageLoader, content: &StatusContent) -> String {
    match content.kind {
        StatusKind::NoFolder => no_folder_opened(loader),
        StatusKind::NoImages => no_images(loader),
        StatusKind::Pages => {
            let mode_label = spread_label(loader, content.spread);
            let dir_label = direction_label(loader, content.direction);
            format!("{}  [{} \u{00b7} {}]", content.pages, mode_label, dir_label)
        }
    }
}

/// Build the notices list from a [`NoticesContent`] snapshot.
pub(crate) fn format_notices(
    loader: &FluentLanguageLoader,
    content: &NoticesContent,
) -> Vec<String> {
    let mut notices = Vec::new();
    if content.skipped > 0 {
        let detail = match content.skipped_detail {
            SkippedDetail::None => String::new(),
            SkippedDetail::Archive => fl!(loader, "notice-skipped-detail-archive"),
        };
        let n = content.skipped as i64;
        notices.push(fl!(
            loader,
            "notice-entries-skipped",
            n = n,
            detail = detail.as_str()
        ));
    }
    if let Some(e_str) = &content.settings_save_err {
        notices.push(failed_save_settings(loader, e_str));
    }
    if let Some(e_str) = &content.library_save_err {
        notices.push(failed_save_library(loader, e_str));
    }
    notices
}

// ---- Tests ----------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::i18n::Localizer;
    use gashuu_core::Language;

    fn en() -> Localizer {
        Localizer::new(Language::En)
    }

    fn ja() -> Localizer {
        Localizer::new(Language::Ja)
    }

    #[test]
    fn spread_labels_differ_across_languages() {
        for mode in [SpreadMode::Single, SpreadMode::Double, SpreadMode::Auto] {
            assert_ne!(
                spread_label(en().loader(), mode),
                spread_label(ja().loader(), mode),
                "spread_label should differ for {:?}",
                mode
            );
        }
        for dir in [ReadingDirection::Ltr, ReadingDirection::Rtl] {
            assert_ne!(
                direction_label(en().loader(), dir),
                direction_label(ja().loader(), dir),
                "direction_label should differ for {:?}",
                dir
            );
        }
    }

    #[test]
    fn parameterized_fns_embed_their_args() {
        for loc in [en(), ja()] {
            let l = loc.loader();
            assert!(open_error(l, &"boom").contains("boom"));
            assert!(page_unavailable(l, 7).contains('7'));
            assert!(decode_error(l, &"bad").contains("bad"));
            assert!(failed_save_settings(l, &"io").contains("io"));
            assert!(failed_save_library(l, &"io").contains("io"));
            assert!(could_not_save_settings(l, &"io").contains("io"));
            assert!(load_failed(l, "settings (x)").contains("settings (x)"));
            assert!(added_books(l, 3).contains('3'));
            assert!(added_books_save_failed(l, 3, &"io").contains('3'));
            assert!(added_books_save_failed(l, 3, &"io").contains("io"));
        }
    }
}
