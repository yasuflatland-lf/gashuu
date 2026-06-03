//! Per-book view preference overrides and their resolution against global settings.
//!
//! `ViewOverride` is the PERSISTED, partial form: each field is `Some(v)` when the
//! user has set that preference for one book, or `None` to inherit the global
//! `Settings` default. `None` here means "inherit the global default" — an active
//! choice, NOT "unknown" (contrast `ReadingProgress::total`). `ResolvedView` is the
//! derived, transient form with every field concrete; it is produced by
//! `ViewOverride::resolve(&Settings)` and never persisted. The merge rule lives in
//! exactly one place (`resolve`) so the per-field fallback is unit-tested once.
//!
//! Headless: no `slint`, no `tracing`.

use crate::settings::{CoverMode, FitMode, ReadingDirection, Settings, SpreadMode};
use serde::{Deserialize, Serialize};

/// Per-book overrides for the four view preferences. Each `None` field inherits
/// the global `Settings` default. Stored as one nested field on `Book`; an
/// all-`None` override serializes to nothing (see the `skip_serializing_if` on
/// each field and on `Book::overrides`), keeping `library.json` byte-compatible
/// with files written before this feature existed. Immutable `Copy` value object.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct ViewOverride {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reading_direction: Option<ReadingDirection>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub spread_mode: Option<SpreadMode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cover_mode: Option<CoverMode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fit_mode: Option<FitMode>,
}

impl ViewOverride {
    /// An all-`None` override (inherit every preference from global). Same as
    /// `Default`, named for intent at call sites that clear a book's overrides.
    pub fn none() -> Self {
        Self::default()
    }

    /// True when every field is `None` (the book inherits all global defaults).
    /// Used as the `skip_serializing_if` predicate on `Book::overrides`.
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }

    /// Merge this override with `global`: a `Some` field wins, a `None` field
    /// falls back to the matching global value. The SINGLE definition of the
    /// per-field fallback rule.
    pub fn resolve(&self, global: &Settings) -> ResolvedView {
        ResolvedView {
            reading_direction: self.reading_direction.unwrap_or(global.reading_direction),
            spread_mode: self.spread_mode.unwrap_or(global.spread_mode),
            cover_mode: self.cover_mode.unwrap_or(global.cover_mode),
            fit_mode: self.fit_mode.unwrap_or(global.fit_mode),
        }
    }
}

/// Fully resolved view preferences for one open book: every field concrete (no
/// `Option`). Derived via `ViewOverride::resolve`; transient (never persisted).
/// Consumed by `ViewerState::apply_resolved_view` and `ViewportState::set_fit`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResolvedView {
    pub reading_direction: ReadingDirection,
    pub spread_mode: SpreadMode,
    pub cover_mode: CoverMode,
    pub fit_mode: FitMode,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn global() -> Settings {
        // A global with every view mode set to a NON-default value, so an
        // inherited field is provably the global value (not a coincidental default).
        Settings {
            reading_direction: ReadingDirection::Rtl,
            spread_mode: SpreadMode::Double,
            cover_mode: CoverMode::Paired,
            fit_mode: FitMode::Actual,
            ..Settings::default()
        }
    }

    #[test]
    fn empty_override_resolves_entirely_to_global() {
        let resolved = ViewOverride::none().resolve(&global());
        assert_eq!(resolved.reading_direction, ReadingDirection::Rtl);
        assert_eq!(resolved.spread_mode, SpreadMode::Double);
        assert_eq!(resolved.cover_mode, CoverMode::Paired);
        assert_eq!(resolved.fit_mode, FitMode::Actual);
    }

    #[test]
    fn set_field_wins_over_global_others_inherit() {
        let ov = ViewOverride {
            reading_direction: Some(ReadingDirection::Ltr),
            ..ViewOverride::none()
        };
        let resolved = ov.resolve(&global());
        // The set field wins...
        assert_eq!(resolved.reading_direction, ReadingDirection::Ltr);
        // ...the unset fields inherit the (non-default) global values.
        assert_eq!(resolved.spread_mode, SpreadMode::Double);
        assert_eq!(resolved.cover_mode, CoverMode::Paired);
        assert_eq!(resolved.fit_mode, FitMode::Actual);
    }

    #[test]
    fn all_fields_set_fully_override_global() {
        let ov = ViewOverride {
            reading_direction: Some(ReadingDirection::Ltr),
            spread_mode: Some(SpreadMode::Single),
            cover_mode: Some(CoverMode::Standalone),
            fit_mode: Some(FitMode::Whole),
        };
        let resolved = ov.resolve(&global());
        assert_eq!(resolved.reading_direction, ReadingDirection::Ltr);
        assert_eq!(resolved.spread_mode, SpreadMode::Single);
        assert_eq!(resolved.cover_mode, CoverMode::Standalone);
        assert_eq!(resolved.fit_mode, FitMode::Whole);
    }

    #[test]
    fn is_empty_tracks_all_none() {
        assert!(ViewOverride::none().is_empty());
        assert!(!ViewOverride {
            spread_mode: Some(SpreadMode::Single),
            ..ViewOverride::none()
        }
        .is_empty());
    }

    #[test]
    fn empty_override_serializes_to_empty_object() {
        // All-None must emit no keys (so the Book-level skip keeps the JSON shape).
        let json = serde_json::to_string(&ViewOverride::none()).unwrap();
        assert_eq!(json, "{}");
    }

    #[test]
    fn partial_override_round_trips_only_set_fields() {
        let ov = ViewOverride {
            reading_direction: Some(ReadingDirection::Rtl),
            ..ViewOverride::none()
        };
        let json = serde_json::to_string(&ov).unwrap();
        assert_eq!(json, r#"{"reading_direction":"rtl"}"#);
        let back: ViewOverride = serde_json::from_str(&json).unwrap();
        assert_eq!(back, ov);
    }

    #[test]
    fn deserializing_absent_fields_yields_none() {
        let ov: ViewOverride = serde_json::from_str("{}").unwrap();
        assert!(ov.is_empty());
    }
}
