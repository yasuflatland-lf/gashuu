use gashuu_core::Language;
use rust_embed::RustEmbed;
use unic_langid::{langid, LanguageIdentifier};

/// Embedded Fluent catalog assets, compiled into the binary from the `i18n/`
/// folder at the crate root. The folder layout is `i18n/{lang}/{domain}.ftl`.
#[derive(RustEmbed)]
#[folder = "i18n"]
pub(crate) struct Localizations;

/// Map a [`Language`] variant to its BCP 47 [`LanguageIdentifier`].
///
/// This exhaustive match is the compile-time gate that replaces
/// `messages.rs`'s exhaustive-match safety for new `Language` variants:
/// adding a new variant without updating this function is a compile error.
pub(crate) fn langid_for(lang: Language) -> LanguageIdentifier {
    // No wildcard arm — a new `Language` variant must fail compilation here.
    match lang {
        Language::En => langid!("en"),
        Language::Ja => langid!("ja"),
    }
}
