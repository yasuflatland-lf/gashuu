//! Rust-side user-facing messages, parameterized per [`Language`].
//!
//! Slint's bundled translations cover only `@tr()` strings inside `.slint`
//! files; the strings composed in Rust (the status line, open/save notices,
//! decode errors) are translated here instead. Every function is an exhaustive
//! match on [`Language`], so adding a language variant is a compile error in
//! each message until its translation is supplied — the same safety the
//! enum-adapter exhaustive matches give the settings indices.
//!
//! Japanese has no grammatical plural, so count-taking messages format the
//! number directly with no plural-form selection. Keep the Japanese terms in
//! lockstep with `translations/ja/LC_MESSAGES/gashuu.po` (e.g. the spread-mode
//! vocabulary), so the status line and the settings dialog speak one language.

use gashuu_core::{Language, ReadingDirection, SpreadMode};
use std::fmt::Display;

/// Status line when no source has been opened yet.
pub(crate) fn msg_no_folder_opened(lang: Language) -> &'static str {
    match lang {
        Language::En => "No folder opened",
        Language::Ja => "フォルダーが開かれていません",
    }
}

/// Status line when the opened source contains no displayable images.
pub(crate) fn msg_no_images(lang: Language) -> &'static str {
    match lang {
        Language::En => "Folder contains no images",
        Language::Ja => "フォルダーに画像がありません",
    }
}

/// Compact spread-mode label for the status line's `[mode · direction]` tail.
pub(crate) fn msg_spread_label(lang: Language, mode: SpreadMode) -> &'static str {
    match (lang, mode) {
        (Language::En, SpreadMode::Single) => "single",
        (Language::En, SpreadMode::Double) => "double",
        (Language::En, SpreadMode::Auto) => "auto",
        (Language::Ja, SpreadMode::Single) => "単ページ",
        (Language::Ja, SpreadMode::Double) => "見開き",
        (Language::Ja, SpreadMode::Auto) => "自動",
    }
}

/// Compact reading-direction label for the status line's `[mode · direction]` tail.
pub(crate) fn msg_direction_label(lang: Language, dir: ReadingDirection) -> &'static str {
    match (lang, dir) {
        (Language::En, ReadingDirection::Ltr) => "LTR",
        (Language::En, ReadingDirection::Rtl) => "RTL",
        (Language::Ja, ReadingDirection::Ltr) => "左→右",
        (Language::Ja, ReadingDirection::Rtl) => "右→左",
    }
}

/// Status line for a failed open (the path did not open as a source).
pub(crate) fn msg_open_error(lang: Language, e: &dyn Display) -> String {
    match lang {
        Language::En => format!("Error: {e}"),
        Language::Ja => format!("エラー: {e}"),
    }
}

/// The archive-open skip-reason suffix appended to the skipped-entries notice.
/// English carries its historical leading space (`"{n} entries skipped{detail}"`);
/// Japanese uses full-width parentheses, which need no separator.
pub(crate) fn msg_skipped_detail_archive(lang: Language) -> &'static str {
    match lang {
        Language::En => " (zip-slip or oversized)",
        Language::Ja => "（zip-slip または上限超過）",
    }
}

/// Notice for entries skipped while opening a source. `detail` is `""` or the
/// language-matched [`msg_skipped_detail_archive`] suffix.
pub(crate) fn msg_entries_skipped(lang: Language, n: usize, detail: &str) -> String {
    match lang {
        Language::En => format!("{n} entries skipped{detail}"),
        Language::Ja => format!("{n} 件のエントリをスキップしました{detail}"),
    }
}

/// Notice when the open-path settings save (recents tracking) failed.
pub(crate) fn msg_failed_save_settings(lang: Language, e: &dyn Display) -> String {
    match lang {
        Language::En => format!("Failed to save settings: {e}"),
        Language::Ja => format!("設定を保存できませんでした: {e}"),
    }
}

/// Notice when the open-path library save failed.
pub(crate) fn msg_failed_save_library(lang: Language, e: &dyn Display) -> String {
    match lang {
        Language::En => format!("Failed to save library: {e}"),
        Language::Ja => format!("ライブラリを保存できませんでした: {e}"),
    }
}

/// Status line when saving settings from the dialog (or an override reset) failed.
pub(crate) fn msg_could_not_save_settings(lang: Language, e: &dyn Display) -> String {
    match lang {
        Language::En => format!("Could not save settings: {e}"),
        Language::Ja => format!("設定を保存できませんでした: {e}"),
    }
}

/// Boot notice when persisted state failed to load. `what` is the technical
/// failure list (e.g. `"settings (...)"`) and stays untranslated: when the
/// settings file itself is corrupt the language preference is unknown anyway.
pub(crate) fn msg_load_failed(lang: Language, what: &str) -> String {
    match lang {
        Language::En => format!("Could not load {what}; starting fresh."),
        Language::Ja => format!("{what} を読み込めませんでした。初期状態で起動します。"),
    }
}

/// Parenthesized marker appended to the status line when the trailing page of
/// a spread failed to decode. `page` is 1-based.
pub(crate) fn msg_page_unavailable(lang: Language, page: usize) -> String {
    match lang {
        Language::En => format!("(page {page} unavailable)"),
        Language::Ja => format!("（ページ {page} は表示できません）"),
    }
}

/// Status line when the leading page of the current spread failed to decode.
pub(crate) fn msg_decode_error(lang: Language, e: &dyn Display) -> String {
    match lang {
        Language::En => format!("Decode error: {e}"),
        Language::Ja => format!("デコードエラー: {e}"),
    }
}

/// Status line when every picked path was already in the library.
pub(crate) fn msg_already_in_library(lang: Language) -> &'static str {
    match lang {
        Language::En => "Already in library \u{2014} no new books added.",
        Language::Ja => "すでにライブラリにあります \u{2014} 新しい本は追加されませんでした。",
    }
}

/// Status line after a successful add. English keeps its historical "(s)"
/// suffix; Japanese needs no plural form.
pub(crate) fn msg_added_books(lang: Language, n: usize) -> String {
    match lang {
        Language::En => format!("Added {n} book(s)"),
        Language::Ja => format!("{n} 冊の本を追加しました"),
    }
}

/// Status line when books were added but the library save failed.
pub(crate) fn msg_added_books_save_failed(lang: Language, n: usize, e: &dyn Display) -> String {
    match lang {
        Language::En => format!("Added {n} book(s), but could not save library: {e}"),
        Language::Ja => {
            format!("{n} 冊の本を追加しましたが、ライブラリを保存できませんでした: {e}")
        }
    }
}

/// Multi-line keyboard-shortcuts reference rendered read-only in the
/// ShortcutsOverlay (opened from the settings footer). Keep BOTH arms in sync
/// with `keymap::map_key`, and the Japanese vocabulary in lockstep with the
/// settings-dialog terms in `translations/ja/LC_MESSAGES/gashuu.po`.
pub(crate) fn msg_key_bindings_help(lang: Language) -> &'static str {
    match lang {
        Language::En => {
            "\
Navigation:
  Space = next page    Backspace = previous page
  Arrows follow the reading direction (LTR: \u{2192} next; RTL: \u{2190} next)

Modes:
  D = spread (single \u{2192} double \u{2192} auto)
  R = reading direction (LTR / RTL)
  C = cover layout (standalone / paired)

Zoom & fit:
  + / - = zoom in / out    0 = reset view    1 = actual size    f = cycle fit

View:
  T = toggle thumbnail strip

Library:
  Up = return to the library"
        }
        Language::Ja => {
            "\
ナビゲーション:
  Space = 次のページ    Backspace = 前のページ
  矢印キーは読む方向に従います (左から右: \u{2192} が次 / 右から左: \u{2190} が次)

モード:
  D = ページ表示 (単ページ \u{2192} 見開き \u{2192} 自動)
  R = 読む方向 (左から右 / 右から左)
  C = 表紙レイアウト (単独 / ペア)

ズームとフィット:
  + / - = ズームイン / アウト    0 = 表示リセット    1 = 原寸    f = フィット切替

表示:
  T = サムネイル一覧の表示切替

ライブラリ:
  Up = ライブラリに戻る"
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every static message must be non-empty in both languages and must
    /// actually differ between them (a copy-pasted untranslated arm is a bug).
    #[test]
    fn static_messages_are_non_empty_and_translated() {
        let statics: [(&str, &str); 4] = [
            (
                msg_no_folder_opened(Language::En),
                msg_no_folder_opened(Language::Ja),
            ),
            (msg_no_images(Language::En), msg_no_images(Language::Ja)),
            (
                msg_skipped_detail_archive(Language::En),
                msg_skipped_detail_archive(Language::Ja),
            ),
            (
                msg_already_in_library(Language::En),
                msg_already_in_library(Language::Ja),
            ),
        ];
        for (en, ja) in statics {
            assert!(!en.is_empty());
            assert!(!ja.is_empty());
            assert_ne!(en, ja);
        }
    }

    #[test]
    fn spread_and_direction_labels_cover_both_languages() {
        for mode in [SpreadMode::Single, SpreadMode::Double, SpreadMode::Auto] {
            assert_ne!(
                msg_spread_label(Language::En, mode),
                msg_spread_label(Language::Ja, mode)
            );
        }
        for dir in [ReadingDirection::Ltr, ReadingDirection::Rtl] {
            assert_ne!(
                msg_direction_label(Language::En, dir),
                msg_direction_label(Language::Ja, dir)
            );
        }
    }

    #[test]
    fn parameterized_messages_embed_their_arguments() {
        for lang in [Language::En, Language::Ja] {
            assert!(msg_open_error(lang, &"boom").contains("boom"));
            assert!(msg_entries_skipped(lang, 42, "").contains("42"));
            assert!(msg_failed_save_settings(lang, &"io").contains("io"));
            assert!(msg_failed_save_library(lang, &"io").contains("io"));
            assert!(msg_could_not_save_settings(lang, &"io").contains("io"));
            assert!(msg_load_failed(lang, "settings (x)").contains("settings (x)"));
            assert!(msg_page_unavailable(lang, 7).contains('7'));
            assert!(msg_decode_error(lang, &"bad").contains("bad"));
            assert!(msg_added_books(lang, 3).contains('3'));
            assert!(msg_added_books_save_failed(lang, 3, &"io").contains('3'));
            assert!(msg_added_books_save_failed(lang, 3, &"io").contains("io"));
        }
    }

    #[test]
    fn english_arms_preserve_the_historical_strings() {
        // These exact strings are pinned by the pre-i18n UI tests and docs;
        // the extraction into this module must not have changed them. The two
        // save-failure messages are pinned SEPARATELY: their English texts
        // differ by design (notice vs dialog phrasing) even though their
        // Japanese arms are deliberately identical.
        assert_eq!(msg_no_folder_opened(Language::En), "No folder opened");
        assert_eq!(msg_no_images(Language::En), "Folder contains no images");
        assert_eq!(
            msg_entries_skipped(Language::En, 3, msg_skipped_detail_archive(Language::En)),
            "3 entries skipped (zip-slip or oversized)"
        );
        assert_eq!(msg_added_books(Language::En, 2), "Added 2 book(s)");
        assert_eq!(msg_decode_error(Language::En, &"x"), "Decode error: x");
        assert_eq!(
            msg_failed_save_settings(Language::En, &"x"),
            "Failed to save settings: x"
        );
        assert_eq!(
            msg_could_not_save_settings(Language::En, &"x"),
            "Could not save settings: x"
        );
    }

    #[test]
    fn key_bindings_help_is_translated_with_matching_shape() {
        let en = msg_key_bindings_help(Language::En);
        let ja = msg_key_bindings_help(Language::Ja);
        assert_ne!(en, ja);
        // Both arms must document the same bindings: equal line counts, so a
        // shortcut added to one arm cannot silently go missing from the other.
        assert_eq!(en.lines().count(), ja.lines().count());
    }

    #[test]
    fn japanese_labels_match_the_po_vocabulary() {
        // The settings dialog translates "Single"/"Double"/"Auto" through the
        // bundled .po while the status line goes through msg_spread_label —
        // two catalogs, one vocabulary. Pin the exact Japanese terms so a
        // translator editing translations/ja/LC_MESSAGES/gashuu.po (or this
        // module) cannot silently make the two screens disagree.
        assert_eq!(
            msg_spread_label(Language::Ja, SpreadMode::Single),
            "単ページ"
        );
        assert_eq!(msg_spread_label(Language::Ja, SpreadMode::Double), "見開き");
        assert_eq!(msg_spread_label(Language::Ja, SpreadMode::Auto), "自動");
    }
}
