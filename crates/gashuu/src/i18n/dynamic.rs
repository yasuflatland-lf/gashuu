//! Fluent-backed dynamic message functions.
//!
//! These functions replace the former `messages.rs` exhaustive-match catalog
//! for all runtime-composed strings (status line, notices, error messages).
//! Each function takes a `&FluentLanguageLoader` borrowed from a [`super::Localizer`]
//! and returns a freshly formatted [`String`] in the active locale.
//!
//! The `format_status` and `format_notices` aggregators apply the active locale to
//! the language-free content structs from `crate::viewer_state` (`StatusContent`)
//! and `crate::app` (`NoticesContent`) respectively; the remaining functions format
//! individual messages from `gashuu_core` types.

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

/// Status line for a failed open whose source file is unreachable — moved, or on
/// a volume that isn't mounted. Names the book and explains in plain language;
/// the raw I/O error is logged in `OpenBookUseCase::run`, not surfaced here.
pub(crate) fn open_inaccessible(loader: &FluentLanguageLoader, title: &str) -> String {
    fl!(loader, "viewer-open-inaccessible", title = title)
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

/// Notice when the NavBar bookmark capsule is clicked but no continue-reading
/// bookmark is registered (or it points at a book no longer in the library).
pub(crate) fn bookmark_none(loader: &FluentLanguageLoader) -> String {
    fl!(loader, "notice-bookmark-none")
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

/// Notice when some books were added but some paths were rejected and skipped.
/// `n` is the number of books successfully added; `skipped` is the number of
/// paths rejected because they could not be opened as a book (no image pages,
/// or unreadable/unsupported); the user-facing wording says "no images" as a
/// deliberate simplification.
pub(crate) fn added_books_skipped(
    loader: &FluentLanguageLoader,
    n: usize,
    skipped: usize,
) -> String {
    let n = n as i64;
    let skipped = skipped as i64;
    fl!(
        loader,
        "notice-added-books-skipped",
        n = n,
        skipped = skipped
    )
}

/// Notice when all picked paths were rejected and nothing was added to the
/// library. `skipped` is the total number of paths rejected because they could
/// not be opened as a book (no image pages, or unreadable/unsupported); the
/// user-facing wording says "no images" as a deliberate simplification.
pub(crate) fn no_books_added_empty(loader: &FluentLanguageLoader, skipped: usize) -> String {
    let skipped = skipped as i64;
    fl!(loader, "notice-no-books-added-empty", skipped = skipped)
}

/// Transient bottom-strip progress while a bulk add probes each picked source
/// off the UI thread: `done` of `total` sources probed so far. Set on every
/// probe completion (issue 206), then overwritten by the final add notice once
/// every source has been classified.
pub(crate) fn adding_progress(loader: &FluentLanguageLoader, done: usize, total: usize) -> String {
    let done = done as i64;
    let total = total as i64;
    fl!(loader, "notice-adding-progress", done = done, total = total)
}

/// Notice when an existing library entry is auto-removed because its source
/// has no images. `title` is the book's display title.
pub(crate) fn empty_book_removed(loader: &FluentLanguageLoader, title: &str) -> String {
    fl!(loader, "notice-empty-book-removed", title = title)
}

// ---- Library screen messages ----------------------------------------------

/// Idle bottom-strip label on the Library screen: the total library book count.
///
/// Returns `String::new()` when `n == 0`: the empty-state panel already explains
/// an empty library, so an idle "0 books" in the strip would be noise. For
/// `n > 0` the count is rendered via the `library-count` Fluent plural select.
pub(crate) fn library_count_text(loader: &FluentLanguageLoader, n: usize) -> String {
    if n == 0 {
        return String::new();
    }
    let n = n as i64;
    fl!(loader, "library-count", n = n)
}

// ---- Selection toolbar messages -------------------------------------------

/// Toolbar count label.
///
/// When `total == 0`, returns the mode-indicator string (the
/// `selection-mode-label` FTL key, e.g. "Selection mode") rather than a count.
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
    if total == 0 {
        return fl!(loader, "selection-mode-label");
    }
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

// ---- Confirm-delete dialog messages ---------------------------------------

/// Dialog title for the bulk-delete confirmation dialog.
/// `n` is the count of books to delete.
pub(crate) fn confirm_delete_title(loader: &FluentLanguageLoader, n: usize) -> String {
    let n = n as i64;
    fl!(loader, "confirm-delete-title", n = n)
}

/// Overflow line when the preview list is truncated.
/// `n` is the count of books not shown in the preview.
pub(crate) fn confirm_delete_more(loader: &FluentLanguageLoader, n: usize) -> String {
    let n = n as i64;
    fl!(loader, "confirm-delete-more", n = n)
}

/// Explanatory body line: what gets removed vs what stays on disk.
pub(crate) fn confirm_delete_keep_files(loader: &FluentLanguageLoader) -> String {
    fl!(loader, "confirm-delete-keep-files")
}

/// Warning shown when the currently open book is included in the delete set.
pub(crate) fn confirm_delete_open_book(loader: &FluentLanguageLoader) -> String {
    fl!(loader, "confirm-delete-open-book")
}

/// Warning shown when the selection spans books outside the current search filter.
/// `n` is the count of books outside the search projection.
pub(crate) fn confirm_delete_outside_search(loader: &FluentLanguageLoader, n: usize) -> String {
    let n = n as i64;
    fl!(loader, "confirm-delete-outside-search", n = n)
}

/// Notice after a successful bulk delete. `n` is the count of books deleted.
pub(crate) fn deleted_books(loader: &FluentLanguageLoader, n: usize) -> String {
    let n = n as i64;
    fl!(loader, "notice-deleted-books", n = n)
}

/// Notice when the library save failed after a bulk delete, meaning nothing
/// was actually removed.
pub(crate) fn delete_save_failed(loader: &FluentLanguageLoader, e: &dyn Display) -> String {
    let error = e.to_string();
    fl!(loader, "notice-delete-save-failed", error = error.as_str())
}

/// SelectionToolbar DangerButton label composed as "Delete (N)…".
///
/// Decision D1: a trailing parenthesized numeral + ellipsis is word-order-safe
/// in both en ("Delete (3)…") and ja ("削除 (3)…") — numerals are exempt from
/// the no-fragment-concatenation rule; only translated WORD fragments are banned.
pub(crate) fn selection_delete_label(loader: &FluentLanguageLoader, n: usize) -> String {
    format!("{} ({})…", fl!(loader, "selection-delete"), n)
}

// ---- Settings data-clearing messages (issue #178) -------------------------

/// Format a byte count as a compact human-readable size (B / KB / MB).
///
/// The unit symbol is intentionally NOT localized — "KB"/"MB" are
/// language-neutral and the surrounding Fluent template owns any translated
/// wording. Uses 1024-based units and one decimal place for KB/MB so e.g.
/// 1536 bytes reads "1.5 KB" instead of collapsing to "1 KB".
fn human_size(bytes: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = 1024 * 1024;
    if bytes < KB {
        format!("{bytes} B")
    } else if bytes < MB {
        format!("{:.1} KB", bytes as f64 / KB as f64)
    } else {
        format!("{:.1} MB", bytes as f64 / MB as f64)
    }
}

/// Status line after the user clears the reading history (library + recents).
pub(crate) fn reading_history_cleared(loader: &FluentLanguageLoader) -> String {
    fl!(loader, "settings-history-cleared")
}

/// Status line when clearing the reading history failed because the library or
/// settings save did not persist.
pub(crate) fn reading_history_clear_failed(loader: &FluentLanguageLoader) -> String {
    fl!(loader, "settings-history-clear-failed")
}

/// Status line after the user clears the on-disk cover cache. `removed_files` is
/// the number of cached cover files deleted (plural-aware in en); `removed_bytes`
/// is the reclaimed disk space, rendered via [`human_size`].
pub(crate) fn cover_cache_cleared(
    loader: &FluentLanguageLoader,
    removed_files: usize,
    removed_bytes: u64,
) -> String {
    let n = removed_files as i64;
    let size = human_size(removed_bytes);
    fl!(
        loader,
        "settings-cache-cleared",
        n = n,
        size = size.as_str()
    )
}

/// Status line when clearing the cover cache failed because the cache directory
/// could not be opened (e.g. no data dir).
pub(crate) fn cover_cache_clear_failed(loader: &FluentLanguageLoader) -> String {
    fl!(loader, "settings-cache-clear-failed")
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
    fn library_count_text_zero_returns_empty() {
        // n == 0 ⇒ "" (the empty-state panel already explains an empty library,
        // so an idle "0 books" would be noise). Both locales return empty.
        for loc in [en(), ja()] {
            let l = loc.loader();
            assert_eq!(
                library_count_text(l, 0),
                "",
                "library_count_text(0) must be empty"
            );
        }
    }

    #[test]
    fn library_count_text_renders_count_per_locale() {
        // n > 0: the count digit must appear, en singular/plural must differ in
        // their unit word, and en != ja.
        let en_one = library_count_text(en().loader(), 1);
        let en_many = library_count_text(en().loader(), 2);
        let ja_one = library_count_text(ja().loader(), 1);
        // Count digit is embedded (guards a dropped { $n } placeholder).
        assert!(
            en_one.contains('1'),
            "en singular must contain 1: {en_one:?}"
        );
        assert!(
            en_many.contains('2'),
            "en plural must contain 2: {en_many:?}"
        );
        assert!(ja_one.contains('1'), "ja must contain 1: {ja_one:?}");
        // English plural select: "1 book" (singular) vs "2 books" (plural).
        assert!(
            en_one.ends_with("book"),
            "en singular must use 'book', got: {en_one:?}"
        );
        assert!(
            en_many.ends_with("books"),
            "en plural must use 'books', got: {en_many:?}"
        );
        // En != Ja for the same count.
        assert_ne!(
            en_one, ja_one,
            "library_count_text must differ between En and Ja"
        );
    }

    #[test]
    fn bookmark_none_is_non_empty_and_translated() {
        // The no-bookmark notice must render in both locales and differ between
        // them (en "No bookmark registered" vs ja「ブックマークが登録されていません」).
        for loc in [en(), ja()] {
            assert!(
                !bookmark_none(loc.loader()).is_empty(),
                "bookmark_none must not be empty"
            );
        }
        assert_ne!(
            bookmark_none(en().loader()),
            bookmark_none(ja().loader()),
            "bookmark_none must differ between En and Ja"
        );
    }

    #[test]
    fn selection_count_text_zero_returns_mode_label() {
        // Boundary: total == 0 ⇒ "Selection mode" label, not "0 selected". Verified by the
        // result lacking a '0' digit and differing from the (3,3) plain-count form.
        for loc in [en(), ja()] {
            let l = loc.loader();
            let result = selection_count_text(l, 0, 0);
            assert!(
                !result.is_empty(),
                "selection_count_text(0,0) must not be empty"
            );
            // The zero branch must NOT render a bare count digit: the mode label
            // contains no ASCII digit (distinguishes it from "0 selected").
            assert!(
                !result.contains('0'),
                "zero form must not contain a digit '0' (got count form, not mode label): {result:?}"
            );
        }
        // En != Ja (different locale strings)
        assert_ne!(
            selection_count_text(en().loader(), 0, 0),
            selection_count_text(ja().loader(), 0, 0),
            "selection_count_text zero form must differ between En and Ja"
        );
        // Zero form must differ from the plain "N selected" form (total=3, visible=3)
        assert_ne!(
            selection_count_text(en().loader(), 0, 0),
            selection_count_text(en().loader(), 3, 3),
            "selection_count_text zero form must differ from plain count form"
        );
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
            assert!(open_inaccessible(l, "銃夢火星戦記 03巻").contains("銃夢火星戦記 03巻"));
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

    #[test]
    fn confirm_delete_fns_are_non_empty_and_embed_args() {
        for loc in [en(), ja()] {
            let l = loc.loader();
            // Parameterized: n must appear
            assert!(confirm_delete_title(l, 5).contains('5'));
            assert!(confirm_delete_more(l, 2).contains('2'));
            // No-arg: must not be empty
            assert!(!confirm_delete_keep_files(l).is_empty());
            assert!(!confirm_delete_open_book(l).is_empty());
            // Parameterized: n must appear
            assert!(confirm_delete_outside_search(l, 3).contains('3'));
        }
        // En != Ja for each key
        assert_ne!(
            confirm_delete_title(en().loader(), 1),
            confirm_delete_title(ja().loader(), 1),
            "confirm_delete_title must differ between En and Ja"
        );
        assert_ne!(
            confirm_delete_more(en().loader(), 1),
            confirm_delete_more(ja().loader(), 1),
            "confirm_delete_more must differ between En and Ja"
        );
        assert_ne!(
            confirm_delete_keep_files(en().loader()),
            confirm_delete_keep_files(ja().loader()),
            "confirm_delete_keep_files must differ between En and Ja"
        );
        assert_ne!(
            confirm_delete_open_book(en().loader()),
            confirm_delete_open_book(ja().loader()),
            "confirm_delete_open_book must differ between En and Ja"
        );
        assert_ne!(
            confirm_delete_outside_search(en().loader(), 1),
            confirm_delete_outside_search(ja().loader(), 1),
            "confirm_delete_outside_search must differ between En and Ja"
        );
    }

    #[test]
    fn delete_notice_fns_are_non_empty_and_embed_args() {
        for loc in [en(), ja()] {
            let l = loc.loader();
            assert!(deleted_books(l, 4).contains('4'));
            assert!(delete_save_failed(l, &"disk full").contains("disk full"));
        }
        // En != Ja
        assert_ne!(
            deleted_books(en().loader(), 2),
            deleted_books(ja().loader(), 2),
            "deleted_books must differ between En and Ja"
        );
        assert_ne!(
            delete_save_failed(en().loader(), &"x"),
            delete_save_failed(ja().loader(), &"x"),
            "delete_save_failed must differ between En and Ja"
        );
    }

    #[test]
    fn selection_delete_label_embeds_count_and_ellipsis() {
        // The composed label must contain the count and end with '…' (ellipsis).
        for loc in [en(), ja()] {
            let l = loc.loader();
            let label = selection_delete_label(l, 3);
            assert!(
                label.contains('3'),
                "selection_delete_label must contain count n=3, got: {label:?}"
            );
            assert!(
                label.ends_with('…'),
                "selection_delete_label must end with '…', got: {label:?}"
            );
        }
        // En != Ja (different base word: "Delete" vs "削除")
        assert_ne!(
            selection_delete_label(en().loader(), 1),
            selection_delete_label(ja().loader(), 1),
            "selection_delete_label must differ between En and Ja"
        );
    }

    #[test]
    fn added_books_skipped_embeds_both_args() {
        // n and skipped must both appear in the output; both locales must be
        // non-empty; en must differ from ja.
        for loc in [en(), ja()] {
            let l = loc.loader();
            let result = added_books_skipped(l, 5, 3);
            assert!(!result.is_empty(), "added_books_skipped must not be empty");
            assert!(
                result.contains('5'),
                "added_books_skipped must contain n=5, got: {result:?}"
            );
            assert!(
                result.contains('3'),
                "added_books_skipped must contain skipped=3, got: {result:?}"
            );
        }
        assert_ne!(
            added_books_skipped(en().loader(), 5, 3),
            added_books_skipped(ja().loader(), 5, 3),
            "added_books_skipped must differ between En and Ja"
        );
    }

    #[test]
    fn adding_progress_embeds_both_counts() {
        // done and total must both appear in the output; both locales must be
        // non-empty; en must differ from ja.
        for loc in [en(), ja()] {
            let l = loc.loader();
            let result = adding_progress(l, 3, 6);
            assert!(!result.is_empty(), "adding_progress must not be empty");
            assert!(
                result.contains('3'),
                "adding_progress must contain done=3, got: {result:?}"
            );
            assert!(
                result.contains('6'),
                "adding_progress must contain total=6, got: {result:?}"
            );
        }
        assert_ne!(
            adding_progress(en().loader(), 3, 6),
            adding_progress(ja().loader(), 3, 6),
            "adding_progress must differ between En and Ja"
        );
    }

    #[test]
    fn no_books_added_empty_embeds_skipped_arg() {
        // skipped must appear in the output; both locales must be non-empty;
        // en must differ from ja.
        for loc in [en(), ja()] {
            let l = loc.loader();
            let result = no_books_added_empty(l, 4);
            assert!(!result.is_empty(), "no_books_added_empty must not be empty");
            assert!(
                result.contains('4'),
                "no_books_added_empty must contain skipped=4, got: {result:?}"
            );
        }
        assert_ne!(
            no_books_added_empty(en().loader(), 4),
            no_books_added_empty(ja().loader(), 4),
            "no_books_added_empty must differ between En and Ja"
        );
    }

    #[test]
    fn human_size_picks_unit_by_magnitude() {
        // Sub-KB stays in bytes; >=KB and >=MB switch units with one decimal.
        assert_eq!(human_size(0), "0 B");
        assert_eq!(human_size(512), "512 B");
        assert_eq!(human_size(1024), "1.0 KB");
        assert_eq!(human_size(1536), "1.5 KB");
        assert_eq!(human_size(1024 * 1024), "1.0 MB");
        assert_eq!(human_size(3 * 1024 * 1024 / 2), "1.5 MB");
    }

    #[test]
    fn reading_history_cleared_is_non_empty_and_translated() {
        for loc in [en(), ja()] {
            assert!(
                !reading_history_cleared(loc.loader()).is_empty(),
                "reading_history_cleared must not be empty"
            );
            assert!(
                !reading_history_clear_failed(loc.loader()).is_empty(),
                "reading_history_clear_failed must not be empty"
            );
        }
        assert_ne!(
            reading_history_cleared(en().loader()),
            reading_history_cleared(ja().loader()),
            "reading_history_cleared must differ between En and Ja"
        );
        assert_ne!(
            reading_history_clear_failed(en().loader()),
            reading_history_clear_failed(ja().loader()),
            "reading_history_clear_failed must differ between En and Ja"
        );
    }

    #[test]
    fn cover_cache_cleared_embeds_count_and_size() {
        // The file count and the human size string must both appear; failure
        // helper non-empty; en/ja differ.
        for loc in [en(), ja()] {
            let l = loc.loader();
            let result = cover_cache_cleared(l, 5, 3 * 1024 * 1024 / 2);
            assert!(!result.is_empty(), "cover_cache_cleared must not be empty");
            assert!(
                result.contains('5'),
                "cover_cache_cleared must contain file count 5, got: {result:?}"
            );
            assert!(
                result.contains("1.5 MB"),
                "cover_cache_cleared must contain the human size, got: {result:?}"
            );
            assert!(
                !cover_cache_clear_failed(l).is_empty(),
                "cover_cache_clear_failed must not be empty"
            );
        }
        assert_ne!(
            cover_cache_cleared(en().loader(), 5, 1024),
            cover_cache_cleared(ja().loader(), 5, 1024),
            "cover_cache_cleared must differ between En and Ja"
        );
        assert_ne!(
            cover_cache_clear_failed(en().loader()),
            cover_cache_clear_failed(ja().loader()),
            "cover_cache_clear_failed must differ between En and Ja"
        );
    }

    #[test]
    fn cover_cache_cleared_singular_vs_plural_in_english() {
        // English plural select: "cached file" (singular) vs "cached files" (plural).
        let one = cover_cache_cleared(en().loader(), 1, 1024);
        let many = cover_cache_cleared(en().loader(), 2, 1024);
        assert!(
            one.contains("cached file ") && !one.contains("cached files"),
            "en singular must use 'file', got: {one:?}"
        );
        assert!(
            many.contains("cached files"),
            "en plural must use 'files', got: {many:?}"
        );
    }

    #[test]
    fn empty_book_removed_embeds_title_arg() {
        // title must appear in the output; both locales must be non-empty;
        // en must differ from ja.
        for loc in [en(), ja()] {
            let l = loc.loader();
            let result = empty_book_removed(l, "My Manga");
            assert!(!result.is_empty(), "empty_book_removed must not be empty");
            assert!(
                result.contains("My Manga"),
                "empty_book_removed must contain title, got: {result:?}"
            );
        }
        assert_ne!(
            empty_book_removed(en().loader(), "My Manga"),
            empty_book_removed(ja().loader(), "My Manga"),
            "empty_book_removed must differ between En and Ja"
        );
    }
}
