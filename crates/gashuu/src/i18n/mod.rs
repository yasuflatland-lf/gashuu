//! Fluent localizer — a thin wrapper around [`FluentLanguageLoader`] that
//! keeps the Fluent catalog in step with the gettext/Slint translation path.
//!
//! [`FluentLanguageLoader`] uses interior mutability for its language state,
//! so `&self` receivers on [`Localizer`] are sufficient; wrapping the whole
//! struct in an `Rc<Localizer>` (rather than `Rc<RefCell<Localizer>>`) is safe
//! and lets Slint callbacks clone the `Rc` without a runtime borrow-check.

mod loader;

use gashuu_core::Language;
use i18n_embed::fluent::{fluent_language_loader, FluentLanguageLoader};
use i18n_embed::LanguageLoader as _;
use loader::{langid_for, Localizations};

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
    /// The fallback ("en") catalog is always loaded first so Fluent can fall
    /// back to English for any message key missing from the requested locale.
    /// The requested language is then layered on top.  If the requested
    /// language IS English only a single load is performed.
    ///
    /// # Panics
    ///
    /// Panics if the embedded catalog assets cannot be loaded.  The catalogs
    /// are compile-time-embedded via `RustEmbed` and `langid_for` is
    /// exhaustive, so a failure here is a programmer error, not a runtime
    /// condition.
    pub(crate) fn new(lang: Language) -> Self {
        let loader = fluent_language_loader!();

        // Load the fallback ("en") catalog first.  `load_languages` does NOT
        // auto-load the fallback; it must be included in the call explicitly
        // or the loader silently has no English strings to fall back to.
        loader
            .load_fallback_language(&Localizations)
            .expect("failed to load Fluent fallback (en) catalog — embedded asset missing");

        // Layer the requested language on top (no-op if it is "en", which is
        // already loaded as the fallback above).
        let requested = langid_for(lang);
        loader
            .load_languages(&Localizations, &[requested])
            .unwrap_or_else(|e| {
                panic!("failed to load Fluent catalog for '{lang:?}': {e}");
            });

        // Disable Unicode bidirectional isolation marks (FSI/PDI) around
        // placeables.  The app is not bidirectional, and the legacy
        // gettext/messages.rs strings are pinned byte-identical by tests;
        // leaving isolation marks enabled would insert invisible codepoints
        // that break those comparisons.
        //
        // NOTE: set_use_isolating has no effect until load_languages has been
        // called at least once (per i18n-embed docs), so this call comes last.
        loader.set_use_isolating(false);

        Self { loader }
    }

    /// Switch the active language to `lang`.
    ///
    /// The fallback catalog remains loaded; only the top layer is replaced.
    /// The same "programmer error" policy as [`new`] applies: a failure to
    /// load a compile-time-embedded asset is a `panic`.
    ///
    /// [`new`]: Localizer::new
    pub(crate) fn switch(&self, lang: Language) {
        let requested = langid_for(lang);
        self.loader
            .load_languages(&Localizations, &[requested])
            .unwrap_or_else(|e| {
                panic!("failed to switch Fluent catalog to '{lang:?}': {e}");
            });
        // Re-apply the isolating setting after every load_languages call
        // (the docs note it takes effect only after load_languages).
        self.loader.set_use_isolating(false);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gashuu_core::Language;
    use std::collections::HashMap;

    // ---- helper -------------------------------------------------------

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

    // ---- test 1: all FTL IDs present in every locale -----------------

    /// Parses both .ftl files and asserts that the message-ID sets are equal in
    /// both directions. Catches missing translations in non-fallback locales
    /// at CI time — before fl!() can hide the gap behind a silent En fallback.
    #[test]
    fn all_ftl_ids_present_in_every_locale() {
        use fluent_syntax::parser::parse;

        let en_src = include_str!("../../i18n/en/gashuu.ftl");
        let ja_src = include_str!("../../i18n/ja/gashuu.ftl");

        let en_ast = parse(en_src).expect("En .ftl failed to parse");
        let ja_ast = parse(ja_src).expect("Ja .ftl failed to parse");

        use fluent_syntax::ast::Entry;
        let en_ids: std::collections::BTreeSet<&str> = en_ast
            .body
            .iter()
            .filter_map(|entry| {
                if let Entry::Message(m) = entry {
                    Some(m.id.name)
                } else {
                    None
                }
            })
            .collect();
        let ja_ids: std::collections::BTreeSet<&str> = ja_ast
            .body
            .iter()
            .filter_map(|entry| {
                if let Entry::Message(m) = entry {
                    Some(m.id.name)
                } else {
                    None
                }
            })
            .collect();

        // IDs in En but missing from Ja
        let missing_in_ja: Vec<&&str> = en_ids.difference(&ja_ids).collect();
        // IDs in Ja but missing from En
        let missing_in_en: Vec<&&str> = ja_ids.difference(&en_ids).collect();

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

    /// Asserts that the Fluent `shortcuts-help` output is byte-identical to
    /// `crate::messages::msg_key_bindings_help` for both languages.
    ///
    /// Empirically verifies that:
    /// (a) Fluent's block-value indentation stripping produces the same
    ///     2-space-indented text as the messages.rs static string arms.
    /// (b) Blank lines between sections are preserved (Fluent gotcha: blank
    ///     continuation lines in block values are kept verbatim, not dropped).
    #[test]
    fn shortcuts_help_matches_legacy_catalog_byte_for_byte() {
        for lang in [Language::En, Language::Ja] {
            let loc = Localizer::new(lang);
            let fluent_text = get(&loc, "shortcuts-help");
            let legacy_text = crate::messages::msg_key_bindings_help(lang);

            assert_eq!(
                fluent_text,
                legacy_text,
                "lang={lang:?}: Fluent 'shortcuts-help' does not match messages.rs arm.\n\
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
    /// corresponding `crate::messages::msg_already_in_library` arms for both
    /// locales, including the em dash (U+2014).
    #[test]
    fn already_in_library_preserves_em_dash() {
        for lang in [Language::En, Language::Ja] {
            let loc = Localizer::new(lang);
            let fluent_text = get(&loc, "notice-already-in-library");
            let legacy_text = crate::messages::msg_already_in_library(lang);

            assert_eq!(
                fluent_text, legacy_text,
                "lang={lang:?}: 'notice-already-in-library' mismatch.\n\
                 Fluent: {fluent_text:?}\n\
                 Legacy: {legacy_text:?}"
            );
        }
    }
}
