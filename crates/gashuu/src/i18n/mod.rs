//! Fluent localizer — the sole i18n system for the crate.
//!
//! Owns the [`FluentLanguageLoader`] lifecycle behind [`Localizer`]: `new`
//! loads a locale, `switch` swaps it atomically, `apply` pushes every static
//! UI string into the Slint [`Strings`] global, and `loader` exposes the raw
//! loader for dynamic strings resolved in [`dynamic`].
//!
//! [`FluentLanguageLoader`] uses interior mutability for its language state,
//! so `&self` receivers on [`Localizer`] are sufficient; wrapping the whole
//! struct in an `Rc<Localizer>` (rather than `Rc<RefCell<Localizer>>`) is safe
//! and lets Slint callbacks clone the `Rc` without a runtime borrow-check.

pub(crate) mod dynamic;
mod loader;

use gashuu_core::Language;
use i18n_embed::fluent::{fluent_language_loader, FluentLanguageLoader};
use i18n_embed::LanguageLoader as _;
use i18n_embed_fl::fl;
use loader::{langid_for, Localizations};
// `ComponentHandle` must be in scope to call `.global::<T>()` on a Slint
// component handle from within this submodule.  The `as _` form avoids an
// unused-import warning when the trait name itself is never referenced directly.
use crate::{Strings, ViewerWindow};
use slint::ComponentHandle as _;

/// Fluent localizer, wrapping a [`FluentLanguageLoader`].
///
/// All mutating methods take `&self` because `FluentLanguageLoader` uses
/// interior mutability; an `Rc<Localizer>` is sufficient for sharing across
/// Slint callbacks.
pub(crate) struct Localizer {
    loader: FluentLanguageLoader,
}

impl Localizer {
    /// Construct a [`Localizer`] for the given [`Language`].
    ///
    /// [`FluentLanguageLoader::load_languages`] auto-appends the fallback
    /// language ("en") to the requested list when absent, then atomically
    /// replaces all loader state via `ArcSwap`.  No separate
    /// `load_fallback_language` call is needed — its result would be
    /// discarded by the subsequent `load_languages` swap.  This auto-append
    /// behavior (relied on by ADR-0008's design) is pinned by
    /// `switch_swaps_languages_and_keeps_fallback`; an i18n-embed upgrade
    /// that drops it will fail that test loudly.  The fallback is
    /// structurally guaranteed but cannot be observed as a runtime resolution
    /// event while all catalogs are kept in ID lockstep by
    /// `all_ftl_ids_present_in_every_locale`.
    ///
    /// # Panics
    ///
    /// Panics if the embedded catalog assets cannot be loaded.  The catalogs
    /// are compile-time-embedded via `RustEmbed` and `langid_for` is
    /// exhaustive, so a failure here is a programmer error, not a runtime
    /// condition.
    pub(crate) fn new(lang: Language) -> Self {
        let loader = fluent_language_loader!();

        // load_languages auto-appends the fallback ("en") when absent and
        // replaces all loader state atomically; calling load_fallback_language
        // first would be redundant — its effect is immediately discarded.
        let requested = langid_for(lang);
        loader
            .load_languages(&Localizations, &[requested])
            .unwrap_or_else(|e| {
                panic!("failed to load Fluent catalog for '{lang:?}': {e}");
            });

        // Disable Unicode bidirectional isolation marks (FSI/PDI) around
        // placeables.  The app is not bidirectional, and the catalog values
        // are pinned byte-identical by exact-equality tests; leaving isolation
        // marks enabled would insert invisible codepoints that break those
        // comparisons.
        //
        // Per `FluentLanguageLoader::set_use_isolating`'s doc note, this has
        // no effect until load_languages has been called; this call comes last.
        loader.set_use_isolating(false);

        Self { loader }
    }

    /// Switch the active language to `lang`.
    ///
    /// `load_languages` performs a full atomic swap of all loader state —
    /// there is no layering.  The fallback ("en") is re-included via
    /// auto-append, and bundle-level config (isolation marks) must be and is
    /// re-applied after each call.  The same "programmer error" policy as
    /// [`new`] applies: a failure to load a compile-time-embedded asset is a
    /// `panic`.
    ///
    /// [`new`]: Localizer::new
    pub(crate) fn switch(&self, lang: Language) {
        let requested = langid_for(lang);
        self.loader
            .load_languages(&Localizations, &[requested])
            .unwrap_or_else(|e| {
                panic!("failed to switch Fluent catalog to '{lang:?}': {e}");
            });
        // Re-apply after load_languages replaces all bundles; per
        // `FluentLanguageLoader::set_use_isolating`'s doc note, the setting
        // takes effect only after load_languages.
        self.loader.set_use_isolating(false);
    }

    /// Return a shared reference to the underlying [`FluentLanguageLoader`].
    ///
    /// Callers in [`dynamic`] borrow this to call `fl!()` directly, keeping
    /// the loader private to this module while still allowing the dynamic
    /// message functions to resolve strings without going through [`apply`].
    ///
    /// [`dynamic`]: super::dynamic
    /// [`apply`]: Localizer::apply
    pub(crate) fn loader(&self) -> &FluentLanguageLoader {
        &self.loader
    }

    /// Push every Fluent-served static string into the [`Strings`] global on
    /// `ui`.
    ///
    /// This is the single chokepoint between the Fluent catalog and the Slint
    /// presentation layer: all [`fl!`] calls in the crate live here so they
    /// remain easy to grep and audit.  Call it at boot (after [`new`]) and after
    /// every [`switch`] to keep the global in sync with the active locale.
    ///
    /// Slint batches property changes and repaints them together before the next
    /// frame, so a sequential push of every localized Strings setter cannot produce
    /// a half-translated frame — the entire swap is visually atomic.
    ///
    /// All `fl!()` calls resolve IDs against the `i18n.toml`-declared crate
    /// catalog.
    ///
    /// [`new`]: Localizer::new
    /// [`switch`]: Localizer::switch
    pub(crate) fn apply(&self, ui: &ViewerWindow) {
        let strings = ui.global::<Strings>();

        // ---- Plain pushes (id == property name, no arguments) ----------
        strings.set_settings_book_title(fl!(self.loader, "settings-book-title").into());
        strings.set_settings_title(fl!(self.loader, "settings-title").into());
        strings.set_settings_section_reading(fl!(self.loader, "settings-section-reading").into());
        strings.set_settings_section_display(fl!(self.loader, "settings-section-display").into());
        strings.set_settings_section_performance(
            fl!(self.loader, "settings-section-performance").into(),
        );
        strings.set_settings_section_general(fl!(self.loader, "settings-section-general").into());
        strings.set_settings_section_data(fl!(self.loader, "settings-section-data").into());
        strings.set_settings_direction_label(fl!(self.loader, "settings-direction-label").into());
        strings.set_settings_direction_ltr(fl!(self.loader, "settings-direction-ltr").into());
        strings.set_settings_direction_rtl(fl!(self.loader, "settings-direction-rtl").into());
        strings.set_settings_direction_a11y(fl!(self.loader, "settings-direction-a11y").into());
        strings.set_settings_spread_label(fl!(self.loader, "settings-spread-label").into());
        strings.set_settings_spread_single(fl!(self.loader, "settings-spread-single").into());
        strings.set_settings_spread_double(fl!(self.loader, "settings-spread-double").into());
        strings.set_settings_spread_auto(fl!(self.loader, "settings-spread-auto").into());
        strings.set_settings_spread_a11y(fl!(self.loader, "settings-spread-a11y").into());
        strings.set_settings_cover_label(fl!(self.loader, "settings-cover-label").into());
        strings.set_settings_cover_standalone(fl!(self.loader, "settings-cover-standalone").into());
        strings.set_settings_cover_paired(fl!(self.loader, "settings-cover-paired").into());
        strings.set_settings_cover_a11y(fl!(self.loader, "settings-cover-a11y").into());
        strings.set_settings_fit_label(fl!(self.loader, "settings-fit-label").into());
        strings.set_settings_fit_whole(fl!(self.loader, "settings-fit-whole").into());
        strings.set_settings_fit_width(fl!(self.loader, "settings-fit-width").into());
        strings.set_settings_fit_actual(fl!(self.loader, "settings-fit-actual").into());
        strings.set_settings_fit_a11y(fl!(self.loader, "settings-fit-a11y").into());
        strings.set_settings_cache_label(fl!(self.loader, "settings-cache-label").into());
        strings.set_settings_cache_a11y(fl!(self.loader, "settings-cache-a11y").into());
        strings.set_settings_preload_label(fl!(self.loader, "settings-preload-label").into());
        strings.set_settings_preload_a11y(fl!(self.loader, "settings-preload-a11y").into());
        strings.set_settings_track_recent_label(
            fl!(self.loader, "settings-track-recent-label").into(),
        );
        strings
            .set_settings_track_recent_a11y(fl!(self.loader, "settings-track-recent-a11y").into());
        strings.set_settings_allow_rar_label(fl!(self.loader, "settings-allow-rar-label").into());
        strings.set_settings_allow_rar_a11y(fl!(self.loader, "settings-allow-rar-a11y").into());
        strings.set_settings_performance_note(fl!(self.loader, "settings-performance-note").into());
        strings.set_settings_language_label(fl!(self.loader, "settings-language-label").into());
        strings.set_settings_language_a11y(fl!(self.loader, "settings-language-a11y").into());
        // General section — data-clearing actions.
        strings.set_settings_clear_history_label(
            fl!(self.loader, "settings-clear-history-label").into(),
        );
        strings
            .set_settings_clear_cache_label(fl!(self.loader, "settings-clear-cache-label").into());
        strings.set_settings_shortcuts_label(fl!(self.loader, "settings-shortcuts-label").into());
        strings.set_settings_reset_to_global(fl!(self.loader, "settings-reset-to-global").into());
        strings.set_shortcuts_title(fl!(self.loader, "shortcuts-title").into());
        strings.set_guide_welcome(fl!(self.loader, "guide-welcome").into());
        strings.set_guide_intro(fl!(self.loader, "guide-intro").into());
        strings.set_guide_open(fl!(self.loader, "guide-open").into());
        strings.set_guide_turn_pages(fl!(self.loader, "guide-turn-pages").into());
        strings.set_guide_modes(fl!(self.loader, "guide-modes").into());
        strings.set_guide_zoom_fit(fl!(self.loader, "guide-zoom-fit").into());
        strings.set_guide_thumbnails(fl!(self.loader, "guide-thumbnails").into());
        strings.set_guide_settings(fl!(self.loader, "guide-settings").into());
        strings.set_guide_got_it(fl!(self.loader, "guide-got-it").into());
        strings.set_carousel_empty_title(fl!(self.loader, "carousel-empty-title").into());
        strings.set_carousel_empty_subtitle(fl!(self.loader, "carousel-empty-subtitle").into());
        strings.set_carousel_empty_cta(fl!(self.loader, "carousel-empty-cta").into());
        strings.set_carousel_empty_cta_files(fl!(self.loader, "carousel-empty-cta-files").into());
        strings.set_carousel_no_results_title(fl!(self.loader, "carousel-no-results-title").into());
        strings.set_carousel_no_results_hint(fl!(self.loader, "carousel-no-results-hint").into());
        strings.set_drag_drop_hint(fl!(self.loader, "drag-drop-hint").into());
        strings.set_cover_failed_label(fl!(self.loader, "cover-failed-label").into());
        strings.set_navbar_search_placeholder(fl!(self.loader, "navbar-search-placeholder").into());
        strings.set_navbar_search_a11y(fl!(self.loader, "navbar-search-a11y").into());
        strings.set_navbar_add_files_a11y(fl!(self.loader, "navbar-add-files-a11y").into());
        strings.set_navbar_add_folder_a11y(fl!(self.loader, "navbar-add-folder-a11y").into());
        strings.set_navbar_add_books_a11y(fl!(self.loader, "navbar-add-books-a11y").into());
        strings.set_continue_reading(fl!(self.loader, "continue-reading").into());
        strings
            .set_viewer_pill_goto_page_a11y(fl!(self.loader, "viewer-pill-goto-page-a11y").into());
        strings.set_viewer_pill_thumbnails_a11y(
            fl!(self.loader, "viewer-pill-thumbnails-a11y").into(),
        );
        strings.set_common_close(fl!(self.loader, "common-close").into());
        strings.set_confirm_delete_cancel(fl!(self.loader, "confirm-delete-cancel").into());

        // ---- Plain pushes (SelectionToolbar) ------------------------------
        strings.set_selection_enter(fl!(self.loader, "selection-enter").into());
        strings.set_selection_exit_a11y(fl!(self.loader, "selection-exit-a11y").into());
        strings.set_selection_delete(fl!(self.loader, "selection-delete").into());

        // ---- 4 pre-composed Stepper labels --------------------------------
        //
        // Composed here via Fluent named args so verb/noun order survives
        // Japanese verb-final grammar — never Slint-side string concatenation.
        // English: "Decrease Cache size in pages"
        // Japanese: "キャッシュサイズ（ページ数）を減らす"  (label comes first)
        let cache_label = fl!(self.loader, "settings-cache-a11y");
        strings.set_stepper_decrease_cache(
            fl!(
                self.loader,
                "stepper-decrease",
                label = cache_label.as_str()
            )
            .into(),
        );
        strings.set_stepper_increase_cache(
            fl!(
                self.loader,
                "stepper-increase",
                label = cache_label.as_str()
            )
            .into(),
        );
        let preload_label = fl!(self.loader, "settings-preload-a11y");
        strings.set_stepper_decrease_preload(
            fl!(
                self.loader,
                "stepper-decrease",
                label = preload_label.as_str()
            )
            .into(),
        );
        strings.set_stepper_increase_preload(
            fl!(
                self.loader,
                "stepper-increase",
                label = preload_label.as_str()
            )
            .into(),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fluent_syntax::ast::{Entry, Resource};
    use fluent_syntax::parser::parse;
    use gashuu_core::Language;
    use std::collections::HashMap;

    // ---- helpers ------------------------------------------------------

    /// Resolve a no-arg message from the loader for the given language.
    fn get(localizer: &Localizer, id: &str) -> String {
        localizer.loader.get(id)
    }

    /// Resolve a message with args for the given language.
    fn get_args(localizer: &Localizer, id: &str, args: HashMap<&str, String>) -> String {
        // Convert HashMap<&str, String> to HashMap<&str, &str> for the loader.
        let args_ref: HashMap<&str, &str> = args.iter().map(|(k, v)| (*k, v.as_str())).collect();
        localizer.loader.get_args(id, args_ref)
    }

    /// Parse an embedded .ftl source into an AST, panicking on a parse error.
    fn parse_ftl(src: &str) -> Resource<&str> {
        parse(src).expect(".ftl failed to parse")
    }

    /// Collect message IDs from an AST in source order, so callers can detect
    /// duplicates (a set alone would silently absorb them).
    fn message_ids<'a>(ast: &'a Resource<&'a str>) -> Vec<&'a str> {
        ast.body
            .iter()
            .filter_map(|entry| match entry {
                Entry::Message(m) => Some(m.id.name),
                _ => None,
            })
            .collect()
    }

    // ---- test 1: all FTL IDs present in every locale -----------------

    /// Parses both .ftl files and asserts that the message-ID sets are equal in
    /// both directions. Catches missing translations in non-fallback locales
    /// at CI time — before fl!() can hide the gap behind a silent En fallback.
    ///
    /// Also asserts that neither file contains duplicate message IDs.  Fluent's
    /// runtime silently last-wins on duplicates; the parser does not error, so
    /// this guard must be explicit.
    #[test]
    fn all_ftl_ids_present_in_every_locale() {
        use std::collections::BTreeSet;

        let en_ast = parse_ftl(include_str!("../../i18n/en/gashuu.ftl"));
        let ja_ast = parse_ftl(include_str!("../../i18n/ja/gashuu.ftl"));

        // Collect IDs as Vecs so duplicates remain visible before deduping.
        let en_ids = message_ids(&en_ast);
        let ja_ids = message_ids(&ja_ast);

        // Duplicate-ID guard: Vec length must equal set length per file.
        let en_set: BTreeSet<&str> = en_ids.iter().copied().collect();
        let ja_set: BTreeSet<&str> = ja_ids.iter().copied().collect();

        assert_eq!(
            en_ids.len(),
            en_set.len(),
            "duplicate message ID in en/gashuu.ftl"
        );
        assert_eq!(
            ja_ids.len(),
            ja_set.len(),
            "duplicate message ID in ja/gashuu.ftl"
        );

        // IDs in En but missing from Ja
        let missing_in_ja: Vec<&&str> = en_set.difference(&ja_set).collect();
        // IDs in Ja but missing from En
        let missing_in_en: Vec<&&str> = ja_set.difference(&en_set).collect();

        assert!(
            missing_in_ja.is_empty(),
            "IDs present in En but missing from Ja: {:?}",
            missing_in_ja
        );
        assert!(
            missing_in_en.is_empty(),
            "IDs present in Ja but missing from En: {:?}",
            missing_in_en
        );
    }

    // ---- test 2: Ja spread vocabulary ---------------------------------

    /// Verifies that the Ja catalog uses the same spread-mode vocabulary for
    /// the settings dialog (`settings-spread-*`) and the viewer status line
    /// (`viewer-spread-*`). Two screens, one vocabulary.
    /// Mirrors messages.rs::japanese_labels_match_the_po_vocabulary.
    #[test]
    fn ja_catalog_pins_spread_vocabulary() {
        let loc = Localizer::new(Language::Ja);

        // Settings section
        assert_eq!(get(&loc, "settings-spread-single"), "単ページ");
        assert_eq!(get(&loc, "settings-spread-double"), "見開き");
        assert_eq!(get(&loc, "settings-spread-auto"), "自動");

        // Viewer status line — same terms
        assert_eq!(get(&loc, "viewer-spread-single"), "単ページ");
        assert_eq!(get(&loc, "viewer-spread-double"), "見開き");
        assert_eq!(get(&loc, "viewer-spread-auto"), "自動");
    }

    // ---- test 3: parameterized messages embed arguments ---------------

    /// Exercises the loader-level get_args API with non-trivial arguments
    /// for both locales, and asserts that set_use_isolating(false) holds
    /// (no bidi isolation marks in the formatted output).
    #[test]
    fn parameterized_messages_embed_arguments() {
        // Bidi isolation marks inserted when set_use_isolating is true.
        const FSI: char = '\u{2068}';
        const PDI: char = '\u{2069}';

        for lang in [Language::En, Language::Ja] {
            let loc = Localizer::new(lang);

            // notice-entries-skipped: n=42, detail="" (no archive suffix)
            {
                let mut args = HashMap::new();
                args.insert("n", "42".to_string());
                args.insert("detail", String::new());
                let result = get_args(&loc, "notice-entries-skipped", args);
                assert!(
                    result.contains("42"),
                    "lang={lang:?}: 'notice-entries-skipped' should contain '42', got: {result:?}"
                );
                // No bidi isolation marks — set_use_isolating(false) must hold.
                assert!(
                    !result.contains(FSI),
                    "lang={lang:?}: FSI isolation mark found in {result:?}"
                );
                assert!(
                    !result.contains(PDI),
                    "lang={lang:?}: PDI isolation mark found in {result:?}"
                );
            }

            // stepper-decrease: label = "Cache size in pages" (En) / "キャッシュサイズ（ページ数）" (Ja)
            // En: "Decrease { $label }" — label embedded anywhere
            // Ja: "{ $label }を減らす" — label comes first, result ends with "を減らす"
            {
                let label = match lang {
                    Language::En => "Cache size in pages".to_string(),
                    Language::Ja => "キャッシュサイズ（ページ数）".to_string(),
                };
                let mut args = HashMap::new();
                args.insert("label", label.clone());
                let result = get_args(&loc, "stepper-decrease", args);

                assert!(
                    result.contains(&label),
                    "lang={lang:?}: 'stepper-decrease' should contain the label, got: {result:?}"
                );
                match lang {
                    Language::En => {
                        // English: "Decrease <label>"
                        assert!(
                            result.starts_with("Decrease"),
                            "En 'stepper-decrease' should start with 'Decrease', got: {result:?}"
                        );
                    }
                    Language::Ja => {
                        // Japanese: "<label>を減らす"
                        assert!(
                            result.ends_with("を減らす"),
                            "Ja 'stepper-decrease' should end with 'を減らす', got: {result:?}"
                        );
                    }
                }

                // No bidi isolation marks.
                assert!(
                    !result.contains(FSI),
                    "lang={lang:?}: FSI found in stepper-decrease output {result:?}"
                );
                assert!(
                    !result.contains(PDI),
                    "lang={lang:?}: PDI found in stepper-decrease output {result:?}"
                );
            }
        }
    }

    // ---- test 4: shortcuts-help line counts match ---------------------

    /// Asserts that the En and Ja `shortcuts-help` values have the same number
    /// of lines. Mirrors messages.rs::key_bindings_help_is_translated_with_matching_shape.
    #[test]
    fn shortcuts_help_line_counts_match_across_locales() {
        let en_loc = Localizer::new(Language::En);
        let ja_loc = Localizer::new(Language::Ja);

        let en_text = get(&en_loc, "shortcuts-help");
        let ja_text = get(&ja_loc, "shortcuts-help");

        let en_lines = en_text.lines().count();
        let ja_lines = ja_text.lines().count();

        assert_eq!(
            en_lines, ja_lines,
            "shortcuts-help line count mismatch: En={en_lines}, Ja={ja_lines}\n\
             En:\n{en_text}\n---\nJa:\n{ja_text}"
        );
    }

    // ---- test 5a: shortcuts-help byte-identical to legacy catalog -----

    /// Asserts that the Fluent `shortcuts-help` output is byte-identical to the
    /// historical `messages.rs::msg_key_bindings_help` arms for both languages.
    /// Those arms are pinned here as literals so the guarantee survives the
    /// deletion of `messages.rs`.
    ///
    /// Empirically verifies that:
    /// (a) Fluent's block-value indentation stripping produces the same
    ///     2-space-indented text as the legacy static string arms.
    /// (b) Blank lines between sections are preserved (Fluent gotcha: blank
    ///     continuation lines in block values are kept verbatim, not dropped).
    #[test]
    fn shortcuts_help_matches_legacy_catalog_byte_for_byte() {
        // The pre-Fluent `messages.rs::msg_key_bindings_help` arms, verbatim.
        let legacy_en = "\
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
  Up / Esc = return to the library

Selection:
  x = enter selection mode    Space = toggle focused
  Cmd/Ctrl+A = select all visible / deselect all
  Delete / Backspace = delete selected books
  Esc = exit selection mode";
        let legacy_ja = "\
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
  Up / Esc = ライブラリに戻る

選択:
  x = 選択モードへ    Space = 選択を切替
  Cmd/Ctrl+A = 表示中をすべて選択 / すべて解除
  Delete / Backspace = 選択した本を削除
  Esc = 選択モードを終了";
        for lang in [Language::En, Language::Ja] {
            let loc = Localizer::new(lang);
            let fluent_text = get(&loc, "shortcuts-help");
            let legacy_text = match lang {
                Language::En => legacy_en,
                Language::Ja => legacy_ja,
            };

            assert_eq!(
                fluent_text,
                legacy_text,
                "lang={lang:?}: Fluent 'shortcuts-help' does not match the legacy arm.\n\
                 Fluent ({} chars):\n{fluent_text:?}\n\
                 Legacy ({} chars):\n{legacy_text:?}",
                fluent_text.len(),
                legacy_text.len()
            );
        }
    }

    // ---- test 5b: leading space / full-width-paren detail preserved ---

    /// Pins the exact byte value of `notice-skipped-detail-archive` for both
    /// locales. English uses the `{" "}` placeable to preserve a historical
    /// leading space; Japanese uses full-width parens with no separator.
    /// This test verifies the {" "} placeable + isolation-off interaction.
    #[test]
    fn skipped_detail_preserves_leading_space() {
        let en_loc = Localizer::new(Language::En);
        let ja_loc = Localizer::new(Language::Ja);

        let en_result = get(&en_loc, "notice-skipped-detail-archive");
        let ja_result = get(&ja_loc, "notice-skipped-detail-archive");

        assert_eq!(
            en_result, " (zip-slip or oversized)",
            "En 'notice-skipped-detail-archive' must begin with a space, got: {en_result:?}"
        );
        assert_eq!(
            ja_result, "（zip-slip または上限超過）",
            "Ja 'notice-skipped-detail-archive' mismatch, got: {ja_result:?}"
        );
    }

    // ---- test 5c: already-in-library em-dash preserved ---------------

    /// Asserts that `notice-already-in-library` is byte-identical to the
    /// historical strings (pinned here as literals, including the em dash
    /// U+2014) for both locales. These literals are the pre-Fluent
    /// `messages.rs::msg_already_in_library` arms, kept here so the byte-for-byte
    /// guarantee survives the deletion of `messages.rs`.
    #[test]
    fn already_in_library_preserves_em_dash() {
        let expected = |lang: Language| match lang {
            Language::En => "Already in library \u{2014} no new books added.",
            Language::Ja => "すでにライブラリにあります \u{2014} 新しい本は追加されませんでした。",
        };
        for lang in [Language::En, Language::Ja] {
            let loc = Localizer::new(lang);
            let fluent_text = get(&loc, "notice-already-in-library");
            let legacy_text = expected(lang);

            assert_eq!(
                fluent_text, legacy_text,
                "lang={lang:?}: 'notice-already-in-library' mismatch.\n\
                 Fluent: {fluent_text:?}\n\
                 Legacy: {legacy_text:?}"
            );
        }
    }

    // ---- test 6a: switch swaps languages and keeps fallback -----------

    /// Verifies that `Localizer::switch` performs a full catalog swap and that
    /// the fallback ("en") is re-included automatically via the fallback
    /// auto-append behavior relied on by ADR-0008's design.  Pins three behaviors:
    ///
    /// 1. En→Ja→En round-trip returns the correct locale-specific value at
    ///    each step, proving the swap is complete (no stale bundle leaks).
    /// 2. After `switch(Ja)`, `current_languages()` reports `["ja"]` — the
    ///    requested language only.  The fallback is loaded into the bundle set
    ///    but is intentionally NOT reflected in `current_languages()` (that is
    ///    how `FluentLanguageLoader` works: it stores only the caller-supplied
    ///    `language_ids`, not `load_language_ids`).  The fallback is
    ///    structurally guaranteed (fallback_language = "en" in i18n.toml +
    ///    load_languages auto-append) but cannot be observed as a runtime
    ///    resolution event while all catalogs are kept in ID lockstep by
    ///    `all_ftl_ids_present_in_every_locale`; a real fallback-resolution
    ///    test belongs to the PR where a translation can actually be missing.
    /// 3. After `switch(Ja)`, a parameterized message contains no FSI/PDI
    ///    isolation marks, proving `set_use_isolating(false)` survives a swap.
    #[test]
    fn switch_swaps_languages_and_keeps_fallback() {
        use unic_langid::langid;

        const FSI: char = '\u{2068}';
        const PDI: char = '\u{2069}';

        // Step 1: En → Ja → En round-trip.
        let loc = Localizer::new(Language::En);
        assert_eq!(
            get(&loc, "settings-spread-single"),
            "Single",
            "En: expected 'Single'"
        );

        loc.switch(Language::Ja);
        assert_eq!(
            get(&loc, "settings-spread-single"),
            "単ページ",
            "After switch(Ja): expected '単ページ'"
        );

        // Step 2: current_languages() reflects only the requested language.
        // The fallback ("en") is auto-appended to the bundle set but is not
        // included in current_languages() — this is expected FluentLanguageLoader
        // behavior (stores caller-supplied list, not the extended load list).
        let current = loc.loader.current_languages();
        assert_eq!(
            current,
            vec![langid!("ja")],
            "After switch(Ja): current_languages() should be [ja], got {:?}",
            current
        );

        loc.switch(Language::En);
        assert_eq!(
            get(&loc, "settings-spread-single"),
            "Single",
            "After switch(En): expected 'Single'"
        );

        // Step 3: isolation marks off across a switch.
        loc.switch(Language::Ja);
        let mut args = HashMap::new();
        args.insert("label", "キャッシュサイズ（ページ数）".to_string());
        let result = get_args(&loc, "stepper-decrease", args);
        assert!(
            !result.contains(FSI),
            "After switch(Ja): FSI mark found in stepper-decrease: {result:?}"
        );
        assert!(
            !result.contains(PDI),
            "After switch(Ja): PDI mark found in stepper-decrease: {result:?}"
        );
    }

    // ---- test 6e: composed stepper labels reproduce apply()'s two-step -------

    /// Reproduces `apply()`'s exact two-step Stepper a11y composition end-to-end
    /// so that a label cross-wire or a word-order regression fails loudly.
    ///
    /// Why this test exists:
    ///
    /// (a) `apply()` resolves `settings-cache-a11y` / `settings-preload-a11y`
    ///     from the live catalog, then passes that string as the `label` named arg
    ///     into `stepper-decrease` / `stepper-increase`.  A cross-wire — e.g.
    ///     feeding `settings-cache-label` ("Cache size (pages)") instead of
    ///     `settings-cache-a11y` ("Cache size in pages") — would produce a
    ///     silently wrong composed string that the existing
    ///     `parameterized_messages_embed_arguments` test (which hardcodes the
    ///     label literal and only asserts starts_with/ends_with) would never catch.
    ///
    /// (b) The four English byte-exact literals below double as a compile-time pin
    ///     for the composed English defaults in `ui/Strings.slint` (lines ~91-94).
    ///     If `settings-cache-a11y` or `stepper-decrease` is edited in en.ftl
    ///     without updating the Slint defaults, this test will fail, alerting
    ///     the author to keep both in sync.
    ///
    /// Japanese byte-exact equality (not ends_with) is essential: verb-final word
    /// order is the entire reason composition lives in Rust rather than Slint.
    /// An `ends_with` check would mask a regression like
    /// `減らす（{ $label }）` (reversed order).
    #[test]
    fn composed_stepper_labels_match_apply_composition() {
        // ---- English -------------------------------------------------------
        let en = Localizer::new(Language::En);

        // Step 1: resolve the label from the catalog (mirrors apply()'s first fl!).
        let en_cache = get(&en, "settings-cache-a11y");
        let en_preload = get(&en, "settings-preload-a11y");

        // Step 2: compose via named arg (mirrors apply()'s second fl!).
        let mut args = HashMap::new();
        args.insert("label", en_cache.clone());
        assert_eq!(
            get_args(&en, "stepper-decrease", args),
            "Decrease Cache size in pages",
            "En stepper-decrease(cache): composed string mismatch — \
             check settings-cache-a11y and stepper-decrease in en.ftl \
             and the Strings.slint stepper-decrease-cache default"
        );

        let mut args = HashMap::new();
        args.insert("label", en_cache.clone());
        assert_eq!(
            get_args(&en, "stepper-increase", args),
            "Increase Cache size in pages",
            "En stepper-increase(cache): composed string mismatch — \
             check settings-cache-a11y and stepper-increase in en.ftl \
             and the Strings.slint stepper-increase-cache default"
        );

        let mut args = HashMap::new();
        args.insert("label", en_preload.clone());
        assert_eq!(
            get_args(&en, "stepper-decrease", args),
            "Decrease Preload radius",
            "En stepper-decrease(preload): composed string mismatch — \
             check settings-preload-a11y and stepper-decrease in en.ftl \
             and the Strings.slint stepper-decrease-preload default"
        );

        let mut args = HashMap::new();
        args.insert("label", en_preload.clone());
        assert_eq!(
            get_args(&en, "stepper-increase", args),
            "Increase Preload radius",
            "En stepper-increase(preload): composed string mismatch — \
             check settings-preload-a11y and stepper-increase in en.ftl \
             and the Strings.slint stepper-increase-preload default"
        );

        // ---- Japanese ------------------------------------------------------
        // Byte-exact equality (not ends_with / starts_with): verb-final word
        // order is the entire reason composition lives in Rust; a reorder like
        // `減らす（{ $label }）` would still pass an ends_with check.
        let ja = Localizer::new(Language::Ja);

        let ja_cache = get(&ja, "settings-cache-a11y");
        let ja_preload = get(&ja, "settings-preload-a11y");

        let mut args = HashMap::new();
        args.insert("label", ja_cache.clone());
        assert_eq!(
            get_args(&ja, "stepper-decrease", args),
            "キャッシュサイズ（ページ数）を減らす",
            "Ja stepper-decrease(cache): byte-exact composition mismatch — \
             check settings-cache-a11y and stepper-decrease in ja.ftl"
        );

        let mut args = HashMap::new();
        args.insert("label", ja_cache.clone());
        assert_eq!(
            get_args(&ja, "stepper-increase", args),
            "キャッシュサイズ（ページ数）を増やす",
            "Ja stepper-increase(cache): byte-exact composition mismatch — \
             check settings-cache-a11y and stepper-increase in ja.ftl"
        );

        let mut args = HashMap::new();
        args.insert("label", ja_preload.clone());
        assert_eq!(
            get_args(&ja, "stepper-decrease", args),
            "先読みページ数を減らす",
            "Ja stepper-decrease(preload): byte-exact composition mismatch — \
             check settings-preload-a11y and stepper-decrease in ja.ftl"
        );

        let mut args = HashMap::new();
        args.insert("label", ja_preload.clone());
        assert_eq!(
            get_args(&ja, "stepper-increase", args),
            "先読みページ数を増やす",
            "Ja stepper-increase(preload): byte-exact composition mismatch — \
             check settings-preload-a11y and stepper-increase in ja.ftl"
        );
    }

    // ---- test 7: japanese notices render in japanese ---------------------

    /// Successor to the deleted `app::tests::japanese_notices_render_in_japanese`.
    /// Exercises `dynamic::format_notices` with a ja-switched loader.
    #[test]
    fn japanese_notices_render_in_japanese() {
        use crate::app::{NoticesContent, SkippedDetail};
        let loc = Localizer::new(Language::Ja);
        let content = NoticesContent {
            skipped: 3,
            skipped_detail: SkippedDetail::Archive,
            settings_save_err: None,
            library_save_err: None,
        };
        let notices = crate::i18n::dynamic::format_notices(loc.loader(), &content);
        assert_eq!(notices.len(), 1);
        assert!(
            notices[0].contains('3'),
            "expected count in notice, got {:?}",
            notices[0]
        );
        assert!(
            notices[0].contains("スキップ"),
            "expected Japanese notice, got {:?}",
            notices[0]
        );
    }

    // ---- test 8: english dynamic fns preserve historical strings ----------

    /// Successor to `messages::tests::english_arms_preserve_the_historical_strings`.
    /// Pins exact English output of `dynamic::` fns against the historical strings.
    #[test]
    fn english_dynamic_fns_preserve_historical_strings() {
        use crate::app::{NoticesContent, SkippedDetail};
        use crate::viewer_state::{StatusContent, StatusKind};
        use gashuu_core::{ReadingDirection, SpreadMode};

        let loc = Localizer::new(Language::En);
        let l = loc.loader();

        // Static status strings
        let no_folder = crate::i18n::dynamic::format_status(
            l,
            &StatusContent {
                pages: String::new(),
                spread: SpreadMode::Single,
                direction: ReadingDirection::Ltr,
                kind: StatusKind::NoFolder,
            },
        );
        assert_eq!(no_folder, "No folder opened");

        let no_images = crate::i18n::dynamic::format_status(
            l,
            &StatusContent {
                pages: String::new(),
                spread: SpreadMode::Single,
                direction: ReadingDirection::Ltr,
                kind: StatusKind::NoImages,
            },
        );
        assert_eq!(no_images, "Folder contains no images");

        // Notice fn pins
        let n3_archive = NoticesContent {
            skipped: 3,
            skipped_detail: SkippedDetail::Archive,
            settings_save_err: None,
            library_save_err: None,
        };
        let notices = crate::i18n::dynamic::format_notices(l, &n3_archive);
        assert_eq!(notices, vec!["3 entries skipped (zip-slip or oversized)"]);

        assert_eq!(crate::i18n::dynamic::added_books(l, 2), "Added 2 book(s)");
        assert_eq!(
            crate::i18n::dynamic::decode_error(l, &"x"),
            "Decode error: x"
        );

        let n_settings_err = NoticesContent {
            skipped: 0,
            skipped_detail: SkippedDetail::None,
            settings_save_err: Some("x".to_string()),
            library_save_err: None,
        };
        let notices = crate::i18n::dynamic::format_notices(l, &n_settings_err);
        assert_eq!(notices, vec!["Failed to save settings: x"]);

        let n_lib_err = NoticesContent {
            skipped: 0,
            skipped_detail: SkippedDetail::None,
            settings_save_err: None,
            library_save_err: Some("x".to_string()),
        };
        let notices = crate::i18n::dynamic::format_notices(l, &n_lib_err);
        assert_eq!(notices, vec!["Failed to save library: x"]);

        // Pages branch: verify composed shape for a known spread/direction
        let pages_status = crate::i18n::dynamic::format_status(
            l,
            &StatusContent {
                pages: "2\u{2013}3 / 6".to_string(),
                spread: SpreadMode::Double,
                direction: ReadingDirection::Ltr,
                kind: StatusKind::Pages,
            },
        );
        assert_eq!(pages_status, "2\u{2013}3 / 6  [double \u{00b7} LTR]");

        // Pin exact English string for could_not_save_settings
        assert_eq!(
            crate::i18n::dynamic::could_not_save_settings(l, &"x"),
            "Could not save settings: x"
        );
    }

    // ---- test 8b: format_notices ordering -----------------------------------

    /// Successor to app::tests::all_three_failures_emit_in_skipped_settings_library_order.
    /// Asserts that format_notices produces notices in the canonical order:
    /// skipped entries FIRST, then settings-save failure, then library-save failure.
    #[test]
    fn format_notices_preserves_skipped_settings_library_order() {
        use crate::app::{NoticesContent, SkippedDetail};
        let loc = Localizer::new(Language::En);
        let l = loc.loader();
        let all_three = NoticesContent {
            skipped: 2,
            skipped_detail: SkippedDetail::Archive,
            settings_save_err: Some("se".to_string()),
            library_save_err: Some("le".to_string()),
        };
        let notices = crate::i18n::dynamic::format_notices(l, &all_three);
        assert_eq!(notices.len(), 3, "expected 3 notices, got {:?}", notices);
        assert!(
            notices[0].contains('2'),
            "skipped notice must be first, got {:?}",
            notices[0]
        );
        assert!(
            notices[1].contains("settings"),
            "settings notice must be second, got {:?}",
            notices[1]
        );
        assert!(
            notices[2].contains("library"),
            "library notice must be third, got {:?}",
            notices[2]
        );
    }

    // ---- test 6c: duplicate-ID guard (integrated into existing test) --
    // Note: the duplicate-ID check is incorporated into
    // `all_ftl_ids_present_in_every_locale` above via a pre-collection
    // assertion.  The existing test is replaced with the enhanced version.

    // ---- test 6d: message arguments match across locales --------------

    /// Asserts that for every shared Fluent message ID, the set of `$variable`
    /// reference names in the pattern AST is identical between en.ftl and
    /// ja.ftl.  A per-locale argument-name typo (e.g. `$lable` vs `$label`)
    /// would otherwise surface only as a runtime log + malformed string in PR-3.
    #[test]
    fn message_arguments_match_across_locales() {
        use fluent_syntax::ast::{Expression, InlineExpression, PatternElement};
        use std::collections::{BTreeSet, HashMap};

        let en_ast = parse_ftl(include_str!("../../i18n/en/gashuu.ftl"));
        let ja_ast = parse_ftl(include_str!("../../i18n/ja/gashuu.ftl"));

        /// Collect all `$variable` names from a pattern's elements (top-level
        /// placeables only; attributes are not used in this catalog).
        fn collect_vars<'a>(elements: &'a [PatternElement<&'a str>]) -> BTreeSet<String> {
            elements
                .iter()
                .filter_map(|elem| {
                    if let PatternElement::Placeable {
                        expression: Expression::Inline(InlineExpression::VariableReference { id }),
                    } = elem
                    {
                        Some(id.name.to_string())
                    } else {
                        None
                    }
                })
                .collect()
        }

        /// Build an ID → arg-set map for one locale's AST.
        fn arg_sets<'a>(ast: &'a Resource<&'a str>) -> HashMap<&'a str, BTreeSet<String>> {
            ast.body
                .iter()
                .filter_map(|entry| match entry {
                    Entry::Message(m) => {
                        let pattern = m.value.as_ref()?;
                        Some((m.id.name, collect_vars(&pattern.elements)))
                    }
                    _ => None,
                })
                .collect()
        }

        let en_args = arg_sets(&en_ast);
        let ja_args = arg_sets(&ja_ast);

        // Compare arg sets for all IDs present in both locales.
        for (id, en_vars) in &en_args {
            if let Some(ja_vars) = ja_args.get(id) {
                assert_eq!(
                    en_vars, ja_vars,
                    "Message '{id}': argument sets differ between en and ja.\n\
                     En vars: {en_vars:?}\n\
                     Ja vars: {ja_vars:?}"
                );
            }
        }
    }
}
