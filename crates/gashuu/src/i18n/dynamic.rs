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

/// Library-screen status when Down is pressed but no book is open (there is
/// no Viewer content to return to, so the navigation is refused).
pub(crate) fn no_open_book(loader: &FluentLanguageLoader) -> String {
    fl!(loader, "viewer-no-open-book")
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
/// flavor has only test callers and is gated to `#[cfg(test)]`.
#[cfg(test)]
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

// ---- Selection toolbar messages -------------------------------------------

/// Toolbar count label.
///
/// When `total > visible_selected`, some selected books are outside the visible
/// projection (filtered out by the current search query). The "(M outside search)"
/// form is shown to prevent silent off-screen deletion — it must appear ONLY when
/// `total` exceeds `visible_selected`.
///
/// When `total == visible_selected` (boundary: all selected books are visible),
/// the plain "N selected" form is used.
pub(crate) fn selection_count_text(
    loader: &FluentLanguageLoader,
    total: usize,
    visible_selected: usize,
) -> String {
    let n = total as i64;
    if total > visible_selected {
        let m = (total - visible_selected) as i64;
        fl!(loader, "selection-count-outside", n = n, m = m)
    } else {
        fl!(loader, "selection-count", n = n)
    }
}

/// Toolbar select-all / deselect-all toggle label.
///
/// Returns the "Deselect all" label when `all_visible_selected` is `true`
/// (all currently visible books are already selected), otherwise returns
/// "Select all".
pub(crate) fn select_all_label(
    loader: &FluentLanguageLoader,
    all_visible_selected: bool,
) -> String {
    if all_visible_selected {
        fl!(loader, "selection-deselect-all")
    } else {
        fl!(loader, "selection-select-all")
    }
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
    fn static_status_strings_are_non_empty_and_translated() {
        // Successor to messages::tests::static_messages_are_non_empty_and_translated
        // for the viewer-no-folder and viewer-no-images IDs.
        for loc in [en(), ja()] {
            let l = loc.loader();
            let no_folder = no_folder_opened(l);
            let no_images = no_images(l);
            let no_open = no_open_book(l);
            assert!(!no_folder.is_empty(), "no_folder_opened must not be empty");
            assert!(!no_images.is_empty(), "no_images must not be empty");
            assert!(!no_open.is_empty(), "no_open_book must not be empty");
        }
        // Differ across locales
        assert_ne!(
            no_folder_opened(en().loader()),
            no_folder_opened(ja().loader()),
            "no_folder_opened must differ between En and Ja"
        );
        assert_ne!(
            no_images(en().loader()),
            no_images(ja().loader()),
            "no_images must differ between En and Ja"
        );
        assert_ne!(
            no_open_book(en().loader()),
            no_open_book(ja().loader()),
            "no_open_book must differ between En and Ja"
        );
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
    fn selection_count_text_plain_when_total_equals_visible() {
        // Boundary: total == visible_selected ⇒ plain "N selected" form.
        for loc in [en(), ja()] {
            let l = loc.loader();
            let result = selection_count_text(l, 3, 3);
            assert!(
                !result.is_empty(),
                "selection_count_text(3,3) must not be empty"
            );
            // n=3 must appear in the rendered output (guards against a dropped { $n }
            // placeholder in the ftl template).
            assert!(
                result.contains('3'),
                "plain form must contain count n=3, got: {result:?}"
            );
        }
        // En != Ja
        assert_ne!(
            selection_count_text(en().loader(), 3, 3),
            selection_count_text(ja().loader(), 3, 3),
            "selection_count_text plain form must differ between En and Ja"
        );
        // Plain form must NOT contain the computed m (0 outside) in English.
        let en_plain = selection_count_text(en().loader(), 3, 3);
        assert!(
            !en_plain.contains("outside"),
            "plain form must not mention 'outside', got: {en_plain:?}"
        );
    }

    #[test]
    fn selection_count_text_outside_form_when_total_greater_than_visible() {
        // total > visible_selected: shows the "outside search" form with computed m.
        // total=5, visible_selected=3 ⇒ m = 5 - 3 = 2.
        for loc in [en(), ja()] {
            let l = loc.loader();
            let result = selection_count_text(l, 5, 3);
            assert!(
                !result.is_empty(),
                "selection_count_text outside form must not be empty"
            );
            // m = 2 must appear in the output.
            assert!(
                result.contains('2'),
                "outside form must contain computed m=2, got: {result:?}"
            );
        }
        // En != Ja
        assert_ne!(
            selection_count_text(en().loader(), 5, 3),
            selection_count_text(ja().loader(), 5, 3),
            "selection_count_text outside form must differ between En and Ja"
        );
    }

    #[test]
    fn select_all_label_flips_between_the_two_keys() {
        // false ⇒ "Select all"; true ⇒ "Deselect all"
        for loc in [en(), ja()] {
            let l = loc.loader();
            let select = select_all_label(l, false);
            let deselect = select_all_label(l, true);
            assert!(
                !select.is_empty(),
                "select_all_label(false) must not be empty"
            );
            assert!(
                !deselect.is_empty(),
                "select_all_label(true) must not be empty"
            );
            assert_ne!(
                select, deselect,
                "select and deselect labels must differ within a locale"
            );
        }
        // En != Ja for both keys
        assert_ne!(
            select_all_label(en().loader(), false),
            select_all_label(ja().loader(), false),
            "select_all_label(false) must differ between En and Ja"
        );
        assert_ne!(
            select_all_label(en().loader(), true),
            select_all_label(ja().loader(), true),
            "select_all_label(true) must differ between En and Ja"
        );
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
