//! View-mode vocabulary: the ubiquitous-language enums for how pages are
//! displayed — reading direction, spread mode/layout, cover pairing, fit, UI
//! language — plus the `KeyBindings` value object and `SpreadMode::resolve`.
//!
//! Extracted from `settings.rs` so the vocabulary is single-owned here and the
//! pure modules that consume it (`spread`, `viewport`, `view_override`) no longer
//! transitively depend on the serde-persistence aggregate. `Settings` (in
//! `settings.rs`) is just one consumer; the external public paths
//! (`gashuu_core::ReadingDirection`, …) are unchanged via `lib.rs` re-exports.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReadingDirection {
    Ltr,
    #[default]
    Rtl,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SpreadMode {
    Single,
    Double,
    #[default]
    Auto, // resolved to Single/Double from window aspect at the UI layer
}

/// A resolved two-page layout decision: exactly Single or Double. `SpreadMode::Auto`
/// is resolved to one of these (via `SpreadMode::resolve`) BEFORE pairing, so the
/// pure `spread::*` functions never see `Auto`. This type carries that invariant
/// in the type system rather than in prose.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpreadLayout {
    Single,
    Double,
}

impl SpreadMode {
    /// Resolve to a concrete `SpreadLayout`. `Single`/`Double` ignore `aspect`
    /// (identity); `Auto` picks `Double` for a landscape-or-square window
    /// (`aspect >= 1.0`) and `Single` for a portrait window. A non-finite or
    /// non-positive `aspect` is treated as `1.0` (=> Double) so a degenerate
    /// window size can never panic or pick a surprising layout.
    pub fn resolve(self, aspect: f32) -> SpreadLayout {
        match self {
            SpreadMode::Single => SpreadLayout::Single,
            SpreadMode::Double => SpreadLayout::Double,
            SpreadMode::Auto => {
                let a = if aspect.is_finite() && aspect > 0.0 {
                    aspect
                } else {
                    1.0
                };
                if a >= 1.0 {
                    SpreadLayout::Double
                } else {
                    SpreadLayout::Single
                }
            }
        }
    }
}

/// How the first page (cover) is laid out in two-page modes (0-based page indices).
/// `Standalone` shows the cover alone (index 0), then pairs from index 1: {1,2}{3,4}…;
/// `Paired` pairs from the cover: {0,1}{2,3}…. Ignored in `Single` mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CoverMode {
    #[default]
    Standalone,
    Paired,
}

/// How a page is scaled to fit the viewport at zoom 1.0. `Whole` contains the
/// whole page (letterboxed); `Width` fills the viewport width (may overflow
/// vertically -> pannable); `Actual` shows pixels 1:1.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FitMode {
    Whole,
    #[default]
    Width,
    Actual,
}

/// UI display language. Global-only (never per-book overridden, so it is NOT
/// part of `ViewOverride`). The snake_case serde tags double as IETF language
/// tags ("en" / "ja"), so the persisted value maps 1:1 onto the locale names
/// the presentation layer's Fluent localizer (`langid_for`) consumes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Language {
    #[default]
    En,
    Ja,
}

/// Key tokens (matching the `.slint` FocusScope tokens) bound to each navigation
/// direction. Persisted in PR3, but `keymap::map_key` hard-codes these same tokens
/// rather than reading this struct; user-remappable keys are deferred to a later PR.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KeyBindings {
    pub next: Vec<String>,
    pub prev: Vec<String>,
}

impl Default for KeyBindings {
    fn default() -> Self {
        Self {
            next: vec!["right".into(), "space".into()],
            prev: vec!["left".into(), "backspace".into()],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_single_double_are_identity() {
        for aspect in [0.5_f32, 1.0, 2.0, f32::NAN, f32::INFINITY] {
            assert_eq!(SpreadMode::Single.resolve(aspect), SpreadLayout::Single);
            assert_eq!(SpreadMode::Double.resolve(aspect), SpreadLayout::Double);
        }
    }

    #[test]
    fn resolve_auto_threshold() {
        // Square or wider => Double; portrait => Single.
        assert_eq!(SpreadMode::Auto.resolve(1.0), SpreadLayout::Double);
        assert_eq!(SpreadMode::Auto.resolve(1.01), SpreadLayout::Double);
        assert_eq!(SpreadMode::Auto.resolve(2.0), SpreadLayout::Double);
        assert_eq!(SpreadMode::Auto.resolve(0.99), SpreadLayout::Single);
        assert_eq!(SpreadMode::Auto.resolve(0.5), SpreadLayout::Single);
    }

    #[test]
    fn resolve_auto_guards_degenerate_aspect() {
        // Non-finite / non-positive aspects are treated as 1.0 (=> Double); no panic.
        for aspect in [0.0_f32, -1.0, f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
            assert_eq!(SpreadMode::Auto.resolve(aspect), SpreadLayout::Double);
        }
    }

    #[test]
    fn spread_mode_auto_round_trips() {
        let json = serde_json::to_string(&SpreadMode::Auto).unwrap();
        assert!(json.contains("auto"), "serialized form was {json:?}");
        let parsed: SpreadMode = serde_json::from_str("\"auto\"").unwrap();
        assert_eq!(parsed, SpreadMode::Auto);
    }
}
