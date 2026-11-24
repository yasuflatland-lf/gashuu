//! Per-book view preference overrides and their resolution against global settings.
//!
//! `ViewOverride` is the PERSISTED, partial form: each field is `Some(v)` when the
//! user has set that preference for one book, or `None` to inherit the global
//! `Settings` default. `None` here means "inherit the global default" — an active
//! choice, NOT "unknown" (contrast `ReadingProgress::total`). `ResolvedView` is the
//! derived, transient form with every field concrete; it is produced by
//! `ViewOverride::resolve(ResolvedView)` and never persisted. The merge rule and its
//! inverse live together in exactly one place (`resolve` and `differences_from`) so
//! the per-field fallback and minimal write-back are each unit-tested once.
//!
//! Headless: no `slint`, no `tracing`.

use crate::view_modes::{CoverMode, FitMode, ReadingDirection, SpreadMode};
use serde::{Deserialize, Serialize};

/// Per-book overrides for the four view preferences. Each `None` field inherits
/// the global `Settings` default. Stored as one nested field on `Book`; an
/// all-`None` override serializes to nothing (see the `skip_serializing_if` on
/// each field and on `Book::overrides`), keeping `library.json` byte-compatible
/// with files written before this feature existed. Immutable `Copy` value object.
///
/// The leave-point write-back records only fields that differ from the global
/// settings, so matching fields remain `None` and continue to inherit later global
/// changes.
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

    /// Merge this override with the global `ResolvedView`: a `Some` field wins, a `None` field
    /// falls back to the matching global value. The SINGLE definition of the
    /// per-field fallback rule.
    pub fn resolve(&self, global: ResolvedView) -> ResolvedView {
        ResolvedView {
            reading_direction: self.reading_direction.unwrap_or(global.reading_direction),
            spread_mode: self.spread_mode.unwrap_or(global.spread_mode),
            cover_mode: self.cover_mode.unwrap_or(global.cover_mode),
            fit_mode: self.fit_mode.unwrap_or(global.fit_mode),
        }
    }

    /// The minimal override that resolves back to `view` against the global `ResolvedView`:
    /// a field equal to its global counterpart is left `None` (inherit).
    /// Invariant: `differences_from(view, global).resolve(global) == view`.
    pub fn differences_from(view: ResolvedView, global: ResolvedView) -> ViewOverride {
        ViewOverride {
            reading_direction: (view.reading_direction != global.reading_direction)
                .then_some(view.reading_direction),
            spread_mode: (view.spread_mode != global.spread_mode).then_some(view.spread_mode),
            cover_mode: (view.cover_mode != global.cover_mode).then_some(view.cover_mode),
            fit_mode: (view.fit_mode != global.fit_mode).then_some(view.fit_mode),
        }
    }
}

/// Fully resolved view preferences for one open book: every field concrete (no
/// `Option`). Derived via `ViewOverride::resolve`; transient (never persisted).
/// Consumed by `ViewerState::apply_resolved_view`; the `fit_mode` field is applied
/// separately by the caller via `ViewportState::set_fit(resolved.fit_mode)`.
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

    fn all_views() -> Vec<ResolvedView> {
        let mut views = Vec::new();
        for reading_direction in [ReadingDirection::Ltr, ReadingDirection::Rtl] {
            for spread_mode in [SpreadMode::Single, SpreadMode::Double, SpreadMode::Auto] {
                for cover_mode in [CoverMode::Standalone, CoverMode::Paired] {
                    for fit_mode in [FitMode::Whole, FitMode::Width, FitMode::Actual] {
                        views.push(ResolvedView {
                            reading_direction,
                            spread_mode,
                            cover_mode,
                            fit_mode,
                        });
                    }
                }
            }
        }
        views
    }

    #[test]
    fn differences_resolve_to_each_view_for_every_global() {
        let views = all_views();
        for &global in &views {
            assert_eq!(ViewOverride::none().resolve(global), global);
            for &view in &views {
                let differences = ViewOverride::differences_from(view, global);
                assert_eq!(differences.resolve(global), view);
                assert_eq!(
                    differences,
                    ViewOverride {
                        reading_direction: (view.reading_direction != global.reading_direction)
                            .then_some(view.reading_direction),
                        spread_mode: (view.spread_mode != global.spread_mode)
                            .then_some(view.spread_mode),
                        cover_mode: (view.cover_mode != global.cover_mode)
                            .then_some(view.cover_mode),
                        fit_mode: (view.fit_mode != global.fit_mode).then_some(view.fit_mode),
                    },
                    "view {view:?}, global {global:?}",
                );
            }
        }
    }

    #[test]
    fn every_override_resolves_set_fields_and_is_empty_only_if_all_are_none() {
        let views = all_views();
        // Every mask chooses which fields are set for each combination of values.
        for &view in &views {
            for mask in 0..16 {
                let overrides = ViewOverride {
                    reading_direction: (mask & 1 != 0).then_some(view.reading_direction),
                    spread_mode: (mask & 2 != 0).then_some(view.spread_mode),
                    cover_mode: (mask & 4 != 0).then_some(view.cover_mode),
                    fit_mode: (mask & 8 != 0).then_some(view.fit_mode),
                };
                assert_eq!(overrides.is_empty(), mask == 0, "{overrides:?}");
                for &global in &views {
                    let resolved = overrides.resolve(global);
                    assert_eq!(
                        resolved.reading_direction,
                        overrides
                            .reading_direction
                            .unwrap_or(global.reading_direction)
                    );
                    assert_eq!(
                        resolved.spread_mode,
                        overrides.spread_mode.unwrap_or(global.spread_mode)
                    );
                    assert_eq!(
                        resolved.cover_mode,
                        overrides.cover_mode.unwrap_or(global.cover_mode)
                    );
                    assert_eq!(
                        resolved.fit_mode,
                        overrides.fit_mode.unwrap_or(global.fit_mode)
                    );
                }
            }
        }
    }

    fn global() -> ResolvedView {
        // Distinct global values exercise inheritance across the four view modes.
        ResolvedView {
            reading_direction: ReadingDirection::Rtl,
            spread_mode: SpreadMode::Double,
            cover_mode: CoverMode::Paired,
            fit_mode: FitMode::Actual,
        }
    }

    #[test]
    fn empty_diff_serializes_to_nothing() {
        let global = global();
        let view = ViewOverride::none().resolve(global);
        let diff = ViewOverride::differences_from(view, global);

        assert_eq!(serde_json::to_string(&diff).unwrap(), "{}");
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
