//! Persistent user settings, serialized to JSON in the OS config directory.
//!
//! Path resolution uses the `directories` crate (`~/.config/gashuu/settings.json`
//! on Linux, the platform equivalents elsewhere). I/O is exposed both as
//! path-taking primitives (`load_from`/`save_to`, testable with `tempfile`) and
//! convenience wrappers (`load`/`save`) that resolve the OS path. This crate stays
//! logging-free: load-failure recovery (including corrupt files) lives in the
//! presentation layer.

use crate::cache::{DEFAULT_CAPACITY, DEFAULT_PREFETCH_RADIUS};
use crate::cache_config::CacheConfig;
use crate::error::CoreError;
use directories::ProjectDirs;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// On-disk schema version. Bump when the shape changes and add a `migrate` step.
pub const SETTINGS_VERSION: u32 = 1;
/// Maximum number of recently opened folders retained.
pub const MAX_RECENT_FILES: usize = 10;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReadingDirection {
    #[default]
    Ltr,
    Rtl,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SpreadMode {
    #[default]
    Single,
    Double,
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
    #[default]
    Whole,
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

/// Persistent user settings. Fields are `#[serde(default)]` so older/partial
/// documents load without error (forward/backward field-add resilience).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Settings {
    #[serde(default = "default_version")]
    pub version: u32,
    #[serde(default)]
    pub reading_direction: ReadingDirection,
    #[serde(default)]
    pub spread_mode: SpreadMode,
    #[serde(default)]
    pub cover_mode: CoverMode,
    #[serde(default)]
    pub fit_mode: FitMode,
    #[serde(default = "default_cache_size")]
    pub cache_size: usize,
    #[serde(default = "default_preload_pages")]
    pub preload_pages: usize,
    #[serde(default)]
    pub key_bindings: KeyBindings,
    /// Record recently opened folders. Off by default (privacy); recent_files is
    /// only updated when this is true.
    #[serde(default)]
    pub track_recent_files: bool,
    /// Recently opened folders, most-recent first. Capped at `MAX_RECENT_FILES`.
    #[serde(default)]
    pub recent_files: Vec<PathBuf>,
    /// Set to `true` after the first-run guide has been shown. Default `false` so
    /// a fresh install shows the guide exactly once.
    #[serde(default)]
    pub seen_guide: bool,
    /// UI display language. `#[serde(default)]` so settings.json files written
    /// before this field existed load as `Language::En` (English).
    #[serde(default)]
    pub language: Language,
}

fn default_version() -> u32 {
    SETTINGS_VERSION
}
fn default_cache_size() -> usize {
    DEFAULT_CAPACITY
}
fn default_preload_pages() -> usize {
    DEFAULT_PREFETCH_RADIUS
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            version: SETTINGS_VERSION,
            reading_direction: ReadingDirection::default(),
            spread_mode: SpreadMode::default(),
            cover_mode: CoverMode::default(),
            fit_mode: FitMode::default(),
            cache_size: DEFAULT_CAPACITY,
            preload_pages: DEFAULT_PREFETCH_RADIUS,
            key_bindings: KeyBindings::default(),
            track_recent_files: false,
            recent_files: Vec::new(),
            seen_guide: false,
            language: Language::default(),
        }
    }
}

impl Settings {
    /// Resolve `settings.json` in the OS config dir (creates nothing).
    pub fn config_path() -> Result<PathBuf, CoreError> {
        let dirs = ProjectDirs::from("", "", "gashuu").ok_or(CoreError::NoConfigDir)?;
        Ok(dirs.config_dir().join("settings.json"))
    }

    /// Load from the OS config path. Missing file → defaults (first run).
    pub fn load() -> Result<Self, CoreError> {
        Self::load_from(&Self::config_path()?)
    }

    /// Load from an explicit path. Missing → defaults; any other I/O error or
    /// malformed JSON → `Err`.
    pub fn load_from(path: &Path) -> Result<Self, CoreError> {
        match std::fs::read_to_string(path) {
            Ok(json) => Self::from_json(&json),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(CoreError::from(e)),
        }
    }

    /// Save to the OS config path (creating parent dirs as needed).
    pub fn save(&self) -> Result<(), CoreError> {
        self.save_to(&Self::config_path()?)
    }

    pub fn save_to(&self, path: &Path) -> Result<(), CoreError> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, self.to_json()?)?;
        Ok(())
    }

    /// Serialize to pretty JSON (also used by the snapshot test).
    pub fn to_json(&self) -> Result<String, CoreError> {
        Ok(serde_json::to_string_pretty(self)?)
    }

    /// Parse JSON, migrating older schema versions to the current shape.
    pub fn from_json(json: &str) -> Result<Self, CoreError> {
        let value: serde_json::Value = serde_json::from_str(json)?;
        if !value.is_object() {
            // Reject non-object roots (e.g. `5`, `[]`, `"x"`, `true`, `null`): `migrate`
            // indexes into the value as a map and would otherwise panic. Surface as a
            // typed error so the presentation layer's corrupt-file recovery handles it.
            // We cannot use `from_value::<Self>` here because all fields carry
            // `#[serde(default)]`, so serde would happily deserialize an array (or other
            // non-object) into an all-defaults Settings — defeating the safety contract.
            // Deserializing into a Map forces serde_json to emit an invalid-type error,
            // which is guaranteed for a non-object value, hence `unwrap_err`.
            let err = serde_json::from_value::<serde_json::Map<String, serde_json::Value>>(value)
                .unwrap_err();
            return Err(err.into());
        }
        // Use a checked conversion instead of a truncating `as u32` cast so that a
        // crafted future-version value (> u32::MAX) is treated as unknown (0) rather
        // than silently wrapping and triggering an unexpected migration.
        let from = value
            .get("version")
            .and_then(|v| v.as_u64())
            .and_then(|n| u32::try_from(n).ok())
            .unwrap_or(0);
        let value = if from < SETTINGS_VERSION {
            migrate(value, from)
        } else {
            value
        };
        let mut settings: Self = serde_json::from_value(value)?;
        // Normalize invariants that a hand-edited or corrupt file could violate.
        // A persisted cache_size of 0 would otherwise be returned verbatim via the
        // public `cache_size` field. `CacheConfig::new` (reached through `cache_config`)
        // owns the `capacity >= 1` floor, so route the stored field through it: this
        // keeps the persisted value equal to the value actually used and keeps the floor
        // defined in exactly one place. (preload_pages is deliberately NOT clamped: 0 is
        // a valid "prefetch disabled" radius and is taken verbatim downstream.)
        settings.cache_size = settings.cache_config().capacity();
        // push_recent caps recent_files on write, but a hand-edited file could exceed
        // MAX_RECENT_FILES and then persist forever (exit-save writes in-memory state);
        // enforce the cap on the read path too.
        settings.recent_files.truncate(MAX_RECENT_FILES);
        Ok(settings)
    }

    /// Record `path` as most-recently-opened when tracking is enabled. Dedups
    /// (moves an existing entry to the front), caps at `MAX_RECENT_FILES`. No-op
    /// when `track_recent_files` is false.
    pub fn push_recent(&mut self, path: PathBuf) {
        if !self.track_recent_files {
            return;
        }
        self.recent_files.retain(|p| p != &path);
        self.recent_files.insert(0, path);
        self.recent_files.truncate(MAX_RECENT_FILES);
    }

    /// Validated cache configuration derived from the persisted `cache_size` /
    /// `preload_pages` fields. This is the canonical way to obtain a `CacheConfig`
    /// from a loaded `Settings`; the `capacity >= 1` floor is guaranteed by
    /// `CacheConfig::new` regardless of the construction site. (The settings-dialog
    /// handlers in `main.rs` edit the raw fields live and rebuild a `CacheConfig`
    /// directly for the in-session update.)
    pub fn cache_config(&self) -> CacheConfig {
        CacheConfig::new(self.cache_size, self.preload_pages)
    }
}

/// Upgrade a raw settings JSON value from `from` to the current schema version.
/// With only v1 today, the sole step documents the v0→v1 contract (v0 predates
/// `preload_pages`); `#[serde(default)]` already covers absent fields, so this is
/// chiefly the hook future schema changes plug into. Stamps the version with the
/// final value reached by the migration chain (which is `SETTINGS_VERSION` once all
/// steps have run).
fn migrate(mut value: serde_json::Value, from: u32) -> serde_json::Value {
    let mut version = from;
    if version == 0 {
        if value.get("preload_pages").is_none() {
            value["preload_pages"] = serde_json::json!(DEFAULT_PREFETCH_RADIUS);
        }
        version = 1;
    }
    value["version"] = serde_json::json!(version);
    value
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_settings_have_expected_values() {
        let s = Settings::default();
        assert_eq!(s.version, 1);
        assert_eq!(s.reading_direction, ReadingDirection::Ltr);
        assert_eq!(s.spread_mode, SpreadMode::Single);
        assert_eq!(s.cover_mode, CoverMode::Standalone);
        assert_eq!(s.fit_mode, FitMode::Whole);
        assert_eq!(s.cache_size, 50);
        assert_eq!(s.preload_pages, 3);
        assert_eq!(s.key_bindings.next, vec!["right", "space"]);
        assert_eq!(s.key_bindings.prev, vec!["left", "backspace"]);
        assert!(!s.track_recent_files);
        assert!(s.recent_files.is_empty());
        assert_eq!(s.language, Language::En);
    }

    fn non_default_settings() -> Settings {
        Settings {
            version: SETTINGS_VERSION,
            reading_direction: ReadingDirection::Rtl,
            spread_mode: SpreadMode::Double,
            cover_mode: CoverMode::Paired,
            fit_mode: FitMode::Width,
            cache_size: 99,
            preload_pages: 7,
            key_bindings: KeyBindings {
                next: vec!["down".into()],
                prev: vec!["up".into()],
            },
            track_recent_files: true,
            recent_files: vec![PathBuf::from("/a"), PathBuf::from("/b")],
            seen_guide: true,
            language: Language::Ja,
        }
    }

    #[test]
    fn round_trip_preserves_all_fields() {
        let original = non_default_settings();
        let json = original.to_json().unwrap();
        let parsed = Settings::from_json(&json).unwrap();
        assert_eq!(original, parsed);
    }

    #[test]
    fn save_to_then_load_from_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        // Path under a non-existent subdir to verify parent auto-creation.
        let path = dir.path().join("nested").join("sub").join("settings.json");
        let original = non_default_settings();
        original.save_to(&path).unwrap();
        let loaded = Settings::load_from(&path).unwrap();
        assert_eq!(original, loaded);
    }

    #[test]
    fn load_from_missing_file_returns_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("does-not-exist.json");
        let loaded = Settings::load_from(&path).unwrap();
        assert_eq!(loaded, Settings::default());
    }

    #[test]
    fn load_from_corrupt_json_errors() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("corrupt.json");
        std::fs::write(&path, "not json").unwrap();
        let err = Settings::load_from(&path).unwrap_err();
        assert!(matches!(err, CoreError::Settings(_)));
    }

    #[test]
    fn migrate_v0_fills_preload_and_bumps_version() {
        let value = serde_json::json!({"version": 0, "cache_size": 7});
        let migrated = migrate(value, 0);
        assert_eq!(
            migrated["preload_pages"].as_u64().unwrap() as usize,
            DEFAULT_PREFETCH_RADIUS
        );
        assert_eq!(migrated["version"].as_u64().unwrap() as u32, 1);
    }

    #[test]
    fn migrate_noop_for_current_version() {
        let original = non_default_settings();
        let json = original.to_json().unwrap();
        let parsed = Settings::from_json(&json).unwrap();
        assert_eq!(original, parsed);
    }

    #[test]
    fn from_json_defaults_missing_fields() {
        let s = Settings::from_json("{\"version\":1}").unwrap();
        assert_eq!(s, Settings::default());
    }

    #[test]
    fn from_json_empty_document_uses_serde_defaults() {
        // A `{}` document exercises the `default_*` serde helpers (version absent
        // triggers migration to v1, while cache_size falls back to its default).
        let s = Settings::from_json("{}").unwrap();
        assert_eq!(s, Settings::default());
    }

    // ── fit_mode tests ──

    #[test]
    fn fit_mode_defaults_to_whole() {
        assert_eq!(FitMode::default(), FitMode::Whole);
        assert_eq!(Settings::default().fit_mode, FitMode::Whole);
    }

    #[test]
    fn fit_mode_round_trip() {
        let s = Settings {
            fit_mode: FitMode::Width,
            ..Default::default()
        };
        let json = s.to_json().unwrap();
        let parsed = Settings::from_json(&json).unwrap();
        assert_eq!(parsed.fit_mode, FitMode::Width);

        let s = Settings {
            fit_mode: FitMode::Actual,
            ..Default::default()
        };
        let json = s.to_json().unwrap();
        let parsed = Settings::from_json(&json).unwrap();
        assert_eq!(parsed.fit_mode, FitMode::Actual);
    }

    #[test]
    fn from_json_missing_fit_mode_defaults_to_whole() {
        // JSON without fit_mode must produce FitMode::Whole via #[serde(default)].
        let json = serde_json::json!({
            "version": SETTINGS_VERSION,
            "reading_direction": "ltr",
            "spread_mode": "single",
            "cover_mode": "standalone",
        })
        .to_string();
        let s = Settings::from_json(&json).unwrap();
        assert_eq!(s.fit_mode, FitMode::Whole);
        // Other fields must be unaffected.
        assert_eq!(s.reading_direction, ReadingDirection::Ltr);
        assert_eq!(s.spread_mode, SpreadMode::Single);
        assert_eq!(s.cover_mode, CoverMode::Standalone);
    }

    #[test]
    fn from_json_unknown_fit_mode_value_errors() {
        // An unknown fit_mode variant (e.g. "auto") is not covered by #[serde(default)],
        // which only supplies a default when the key is absent. serde rejects an
        // unrecognised variant, so from_json must return Err(CoreError::Settings(_)).
        let json = serde_json::json!({
            "version": SETTINGS_VERSION,
            "fit_mode": "auto",
        })
        .to_string();
        let result = Settings::from_json(&json);
        assert!(
            matches!(result, Err(CoreError::Settings(_))),
            "expected Err(CoreError::Settings(_)) for unknown fit_mode value, got {:?}",
            result
        );
    }

    #[test]
    fn push_recent_disabled_is_noop() {
        let mut s = Settings::default();
        assert!(!s.track_recent_files);
        s.push_recent(PathBuf::from("/some/path"));
        assert!(s.recent_files.is_empty());
    }

    #[test]
    fn push_recent_dedups_moves_to_front_and_caps() {
        let mut s = Settings {
            track_recent_files: true,
            ..Default::default()
        };

        // Pushing the same path twice dedups to a single entry.
        s.push_recent(PathBuf::from("/dup"));
        s.push_recent(PathBuf::from("/dup"));
        assert_eq!(s.recent_files.len(), 1);

        // Push more than MAX_RECENT_FILES distinct paths; cap is enforced and
        // the most-recent push is at the front.
        s.recent_files.clear();
        for i in 0..(MAX_RECENT_FILES + 5) {
            s.push_recent(PathBuf::from(format!("/path/{i}")));
        }
        assert_eq!(s.recent_files.len(), MAX_RECENT_FILES);
        let last = MAX_RECENT_FILES + 5 - 1;
        assert_eq!(s.recent_files[0], PathBuf::from(format!("/path/{last}")));
    }

    #[test]
    fn config_path_targets_gashuu_settings_json() {
        let path = Settings::config_path().unwrap();
        assert!(path.ends_with("settings.json"));
        assert!(path.to_string_lossy().contains("gashuu"));
    }

    #[test]
    fn default_settings_json_snapshot() {
        insta::assert_snapshot!(Settings::default().to_json().unwrap());
    }

    // ── FIX 1 regression: non-object JSON roots must return Err, never panic ──

    #[test]
    fn from_json_non_object_root_errors() {
        for input in &["5", "[]", "\"x\"", "true", "null"] {
            let result = Settings::from_json(input);
            assert!(
                matches!(result, Err(CoreError::Settings(_))),
                "expected Err(CoreError::Settings(_)) for input {:?}, got {:?}",
                input,
                result
            );
        }
    }

    // ── FIX 3 regression: recent_files capped on load ──

    #[test]
    fn from_json_caps_recent_files_on_load() {
        let entries: Vec<String> = (0..(MAX_RECENT_FILES + 5))
            .map(|i| format!("/path/{i}"))
            .collect();
        let json = serde_json::json!({
            "version": SETTINGS_VERSION,
            "recent_files": entries,
        })
        .to_string();
        let s = Settings::from_json(&json).unwrap();
        assert_eq!(s.recent_files.len(), MAX_RECENT_FILES);
    }

    // ── FIX 3 regression: cache_size=0 normalized to 1; preload_pages=0 kept ──

    #[test]
    fn load_normalizes_zero_cache_size_to_one() {
        let json = serde_json::json!({
            "version": SETTINGS_VERSION,
            "cache_size": 0,
            "preload_pages": 0,
        })
        .to_string();
        let s = Settings::from_json(&json).unwrap();
        assert_eq!(s.cache_size, 1, "cache_size=0 must be normalized to 1");
        assert_eq!(s.preload_pages, 0, "preload_pages=0 must NOT be clamped");
    }

    // ── push_recent ordering: promote an existing middle entry ──

    #[test]
    fn push_recent_promotes_existing_middle_entry() {
        let mut s = Settings {
            track_recent_files: true,
            recent_files: vec![
                PathBuf::from("/a"),
                PathBuf::from("/b"),
                PathBuf::from("/c"),
            ],
            ..Default::default()
        };
        s.push_recent(PathBuf::from("/b"));
        assert_eq!(
            s.recent_files,
            vec![
                PathBuf::from("/b"),
                PathBuf::from("/a"),
                PathBuf::from("/c"),
            ],
            "existing middle entry must be moved to front, others shifted back"
        );
    }

    // ── migrate guard: present preload_pages must not be overwritten ──

    #[test]
    fn migrate_preserves_existing_preload_pages() {
        let value = serde_json::json!({"version": 0, "preload_pages": 99});
        let migrated = migrate(value, 0);
        assert_eq!(
            migrated["preload_pages"].as_u64().unwrap(),
            99,
            "migrate must not overwrite a present preload_pages value"
        );
        assert_eq!(
            migrated["version"].as_u64().unwrap() as u32,
            1,
            "migrate must stamp version=1 after the v0→v1 step"
        );
    }

    // ── SpreadMode::resolve → SpreadLayout (PR4a) ──

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

    // ── seen_guide tests ──

    #[test]
    fn seen_guide_defaults_to_false() {
        assert!(!Settings::default().seen_guide);
    }

    #[test]
    fn seen_guide_round_trips() {
        let s = Settings {
            seen_guide: true,
            ..Default::default()
        };
        let json = s.to_json().unwrap();
        let parsed = Settings::from_json(&json).unwrap();
        assert!(parsed.seen_guide);
    }

    #[test]
    fn from_json_missing_seen_guide_defaults_to_false() {
        // A JSON object that omits `seen_guide` must produce `false` via
        // `#[serde(default)]`, identical to how `cover_mode`/`fit_mode` were added.
        let s = Settings::from_json(r#"{"version":1}"#).unwrap();
        assert!(
            !s.seen_guide,
            "seen_guide must default to false when absent from JSON"
        );
    }

    // ── language tests ──

    #[test]
    fn language_defaults_to_en() {
        assert_eq!(Language::default(), Language::En);
        assert_eq!(Settings::default().language, Language::En);
    }

    #[test]
    fn language_round_trips() {
        for lang in [Language::En, Language::Ja] {
            let s = Settings {
                language: lang,
                ..Default::default()
            };
            let json = s.to_json().unwrap();
            let parsed = Settings::from_json(&json).unwrap();
            assert_eq!(parsed.language, lang);
        }
    }

    #[test]
    fn language_serializes_as_ietf_tag() {
        // The snake_case serde tags must stay "en"/"ja" — the presentation layer's
        // Fluent localizer (`langid_for`) consumes them as locale identifiers.
        let json = Settings {
            language: Language::Ja,
            ..Default::default()
        }
        .to_json()
        .unwrap();
        assert!(json.contains("\"language\": \"ja\""), "json was {json}");
    }

    #[test]
    fn from_json_missing_language_defaults_to_en() {
        // A JSON object that omits `language` must produce `En` via
        // `#[serde(default)]`, identical to how `seen_guide` was added.
        let s = Settings::from_json(r#"{"version":1}"#).unwrap();
        assert_eq!(s.language, Language::En);
    }

    #[test]
    fn from_json_unknown_language_value_errors() {
        // An unrecognised variant is rejected by serde (mirrors the fit_mode
        // unknown-value contract); #[serde(default)] only covers an ABSENT key.
        let json = serde_json::json!({
            "version": SETTINGS_VERSION,
            "language": "fr",
        })
        .to_string();
        let result = Settings::from_json(&json);
        assert!(
            matches!(result, Err(CoreError::Settings(_))),
            "expected Err(CoreError::Settings(_)) for unknown language value, got {:?}",
            result
        );
    }

    #[test]
    fn cache_config_reflects_fields() {
        let s = Settings {
            cache_size: 30,
            preload_pages: 5,
            ..Default::default()
        };
        let cfg = s.cache_config();
        assert_eq!(cfg.capacity(), 30);
        assert_eq!(cfg.radius(), 5);
    }

    #[test]
    fn cache_config_capacity_is_at_least_one() {
        // The value object clamps capacity even if the raw field somehow held 0.
        let s = Settings {
            cache_size: 0,
            ..Default::default()
        };
        assert_eq!(s.cache_config().capacity(), 1);
        assert_eq!(s.cache_config().radius(), s.preload_pages);
    }

    #[test]
    fn from_json_old_flat_keys_load_into_cache_config() {
        // Back-compat: an existing settings.json carrying the flat cache_size /
        // preload_pages keys still loads and yields a matching CacheConfig.
        let json = serde_json::json!({
            "version": SETTINGS_VERSION,
            "cache_size": 12,
            "preload_pages": 4,
        })
        .to_string();
        let cfg = Settings::from_json(&json).unwrap().cache_config();
        assert_eq!(cfg.capacity(), 12);
        assert_eq!(cfg.radius(), 4);
    }
}
