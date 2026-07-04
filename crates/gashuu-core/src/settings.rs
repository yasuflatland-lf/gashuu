//! Persistent user settings, serialized to JSON in the OS config directory.
//!
//! Path resolution uses the `directories` crate (`~/.config/gashuu/settings.json`
//! on Linux, the platform equivalents elsewhere). I/O is exposed both as
//! path-taking primitives (`load_from`/`save_to`, testable with `tempfile`) and
//! convenience wrappers (`load`/`save`) that resolve the OS path. This crate stays
//! logging-free: load-failure recovery (including corrupt files) lives in the
//! presentation layer.

use crate::archive_loader::ArchivePolicy;
use crate::cache::{DEFAULT_CAPACITY, DEFAULT_PREFETCH_RADIUS};
use crate::cache_config::CacheConfig;
use crate::error::CoreError;
use crate::view_modes::{CoverMode, FitMode, KeyBindings, Language, ReadingDirection, SpreadMode};
use crate::window_geometry::WindowGeometry;
use directories::ProjectDirs;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// On-disk schema version. Bump when the shape changes and add a `migrate` step.
pub const SETTINGS_VERSION: u32 = 1;
/// Maximum number of recently opened folders retained.
pub const MAX_RECENT_FILES: usize = 10;

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
    /// UI display language. `#[serde(default)]` so settings.json files written
    /// before this field existed load as `Language::En` (English).
    #[serde(default)]
    pub language: Language,
    /// When `true` (the default), RAR/CBR archives are opened directly; set to
    /// `false` to reject them at open time. `#[serde(default = "default_allow_rar")]`
    /// ensures both fresh installs and settings files written before this field
    /// existed load as `true`, while a user's explicit `false` is preserved.
    #[serde(default = "default_allow_rar")]
    pub allow_rar_archives: bool,
    /// Last window geometry (size + position, physical pixels). `None` on a fresh
    /// install → the Slint `preferred-*` boot size applies. `skip_serializing_if`
    /// keeps a `None` out of the document, so the default snapshot is unchanged.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub window: Option<WindowGeometry>,
    /// When `true` (the default), check GitHub Releases for a newer version on
    /// startup (throttled to once per 24h). `default_true` so files written
    /// before this field existed adopt the enabled default.
    #[serde(default = "default_true")]
    pub auto_update_check: bool,
    /// A version the user chose to skip; that version is never re-notified.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub skipped_version: Option<String>,
    /// UNIX seconds of the last automatic update check (for the 24h throttle).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_update_check: Option<i64>,
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
fn default_allow_rar() -> bool {
    true
}
fn default_true() -> bool {
    true
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
            language: Language::default(),
            allow_rar_archives: true,
            window: None,
            auto_update_check: true,
            skipped_version: None,
            last_update_check: None,
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

    /// Atomically write the settings to `path`, creating parent directories as needed.
    pub fn save_to(&self, path: &Path) -> Result<(), CoreError> {
        crate::atomic_write::write_atomic(path, self.to_json()?.as_bytes())
    }

    /// Derive the archive-open policy from these settings.
    pub fn archive_policy(&self) -> ArchivePolicy {
        ArchivePolicy {
            allow_rar: self.allow_rar_archives,
        }
    }

    /// Serialize to pretty JSON (also used by the snapshot test).
    pub fn to_json(&self) -> Result<String, CoreError> {
        Ok(serde_json::to_string_pretty(self)?)
    }

    /// Parse JSON, migrating older schema versions to the current shape.
    pub fn from_json(json: &str) -> Result<Self, CoreError> {
        // Non-object guard + version resolution + migrate dispatch are single-homed in
        // `persist`. `CoreError::Settings` is `#[from]`, so both `?`s convert automatically.
        let value = crate::persist::parse_versioned_object(json, SETTINGS_VERSION, migrate)?;
        let mut settings: Self = serde_json::from_value(value)?;
        settings.normalize();
        Ok(settings)
    }

    /// Re-establish domain invariants on a freshly built or loaded `Settings`,
    /// so that a value constructed any way other than the normal write path
    /// (a hand-edited file, a future loader, an in-memory value) still obeys
    /// the same bounds. Mirrors `Library::normalize`. Idempotent.
    ///
    /// Route the cache fields through `CacheConfig::new` (via `cache_config()`),
    /// which owns the `[1, MAX_CACHE_SIZE]` / `[0, MAX_PREFETCH_RADIUS]` clamps.
    /// This keeps the persisted values equal to the values actually used and
    /// keeps the bounds defined in exactly one place. (`preload_pages = 0`
    /// remains valid as a "prefetch disabled" sentinel; values above
    /// `MAX_PREFETCH_RADIUS` are clamped.)
    pub fn normalize(&mut self) {
        let cfg = self.cache_config();
        self.cache_size = cfg.capacity();
        self.preload_pages = cfg.radius();
        // push_recent caps recent_files on write, but a hand-edited file could exceed
        // MAX_RECENT_FILES and persist forever (exit-save writes in-memory state); cap here.
        self.recent_files.truncate(MAX_RECENT_FILES);
        // Normalize a stored window geometry: a corrupt (inflated) size is discarded (boot
        // at default, not off-screen); an otherwise-sane size is floored to the minimum.
        match self.window {
            Some(g) if !g.is_size_sane() => self.window = None,
            Some(ref mut g) => {
                let (w, h) = g.clamped_size();
                g.width = w;
                g.height = h;
            }
            None => {}
        }
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
    /// the settings handlers in the UI crate edit the raw fields live and rebuild
    /// a `CacheConfig` directly for the in-session update.)
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
    use crate::window_geometry::{WindowGeometry, MIN_WINDOW_HEIGHT, MIN_WINDOW_WIDTH};

    #[test]
    fn default_settings_have_expected_values() {
        let s = Settings::default();
        assert_eq!(s.version, 1);
        assert_eq!(s.reading_direction, ReadingDirection::Rtl);
        assert_eq!(s.spread_mode, SpreadMode::Auto);
        assert_eq!(s.cover_mode, CoverMode::Standalone);
        assert_eq!(s.fit_mode, FitMode::Whole);
        assert_eq!(s.cache_size, 50);
        assert_eq!(s.preload_pages, 3);
        assert_eq!(s.key_bindings.next, vec!["right", "space"]);
        assert_eq!(s.key_bindings.prev, vec!["left", "backspace"]);
        assert!(!s.track_recent_files);
        assert!(s.recent_files.is_empty());
        assert!(
            s.allow_rar_archives,
            "allow_rar_archives must default to true"
        );
        assert_eq!(s.language, Language::En);
        assert!(
            s.auto_update_check,
            "auto_update_check must default to true"
        );
        assert_eq!(s.skipped_version, None);
        assert_eq!(s.last_update_check, None);
    }

    fn non_default_settings() -> Settings {
        // Every field must DIFFER from `Settings::default()`, or the round-trip tests pass
        // even when save/load drops a field (the default would re-supply the value).
        Settings {
            version: SETTINGS_VERSION,
            reading_direction: ReadingDirection::Ltr,
            spread_mode: SpreadMode::Double,
            cover_mode: CoverMode::Paired,
            fit_mode: FitMode::Whole,
            cache_size: 99,
            preload_pages: 4,
            key_bindings: KeyBindings {
                next: vec!["down".into()],
                prev: vec!["up".into()],
            },
            track_recent_files: true,
            recent_files: vec![PathBuf::from("/a"), PathBuf::from("/b")],
            language: Language::Ja,
            // Differs from the new default (`true`) so the round-trip tests below
            // would catch save/load dropping this field.
            allow_rar_archives: false,
            window: Some(WindowGeometry {
                width: 1024,
                height: 768,
                x: 120,
                y: -40,
            }),
            // Differ from defaults (true / None / None) so round-trip tests are not vacuous.
            auto_update_check: false,
            skipped_version: Some("v9.9.9".to_string()),
            last_update_check: Some(1_700_000_000),
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
    fn save_to_creates_missing_parent_directories() {
        // The atomic helper owns parent creation; saving under a non-existent
        // subtree must materialize the directories AND the file.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("deep").join("nest").join("settings.json");
        assert!(!path.parent().unwrap().exists());
        Settings::default().save_to(&path).unwrap();
        assert!(path.exists());
    }

    #[test]
    fn save_to_overwrites_existing_file_with_complete_json() {
        // Saving over an existing (longer) settings file must replace it wholesale
        // with the new document — no truncation, no leftover tail from the old one.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");

        // First save: a fully-populated (longer) document.
        non_default_settings().save_to(&path).unwrap();

        // Second save: defaults (a shorter document on most fields).
        let replacement = Settings::default();
        replacement.save_to(&path).unwrap();

        // The bytes on disk must equal exactly the new serialization, and parse
        // back to the replacement value with no residue from the first write.
        let on_disk = std::fs::read_to_string(&path).unwrap();
        assert_eq!(on_disk, replacement.to_json().unwrap());
        assert_eq!(Settings::load_from(&path).unwrap(), replacement);
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
        // Both legs use NON-default variants (default is Whole) so the round trip
        // cannot pass by the parser merely re-defaulting a dropped field.
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
        // The explicit keys must parse as written — "ltr"/"single" are NON-default
        // variants, so these also prove explicit values win over the defaults.
        assert_eq!(s.reading_direction, ReadingDirection::Ltr);
        assert_eq!(s.spread_mode, SpreadMode::Single);
        assert_eq!(s.cover_mode, CoverMode::Standalone);
    }

    #[test]
    fn from_json_unknown_fit_mode_value_errors() {
        // An unknown fit_mode variant isn't covered by #[serde(default)] (which only fills an
        // ABSENT key); serde rejects it, so from_json returns Err(CoreError::Settings(_)).
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
        // `#[serde(default)]`, identical to how `cover_mode`/`fit_mode` were added.
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

    #[test]
    fn from_json_clamps_cache_size_above_max() {
        use crate::cache_config::MAX_CACHE_SIZE;
        let json = serde_json::json!({
            "version": SETTINGS_VERSION,
            "cache_size": MAX_CACHE_SIZE + 50,
        })
        .to_string();
        let s = Settings::from_json(&json).unwrap();
        assert_eq!(s.cache_size, MAX_CACHE_SIZE);
    }

    #[test]
    fn from_json_clamps_preload_pages_above_max() {
        use crate::cache_config::MAX_PREFETCH_RADIUS;
        let json = serde_json::json!({
            "version": SETTINGS_VERSION,
            "preload_pages": MAX_PREFETCH_RADIUS + 10,
        })
        .to_string();
        let s = Settings::from_json(&json).unwrap();
        assert_eq!(s.preload_pages, MAX_PREFETCH_RADIUS);
    }

    // ── window geometry tests ──

    #[test]
    fn window_defaults_to_none() {
        assert_eq!(Settings::default().window, None);
    }

    #[test]
    fn window_omitted_from_default_json() {
        // skip_serializing_if keeps a None window out of the serialized document.
        let json = Settings::default().to_json().unwrap();
        assert!(
            !json.contains("window"),
            "a None window must not appear in settings.json, got {json}"
        );
    }

    #[test]
    fn window_round_trips() {
        let s = Settings {
            window: Some(WindowGeometry {
                width: 1024,
                height: 768,
                x: 120,
                y: -40,
            }),
            ..Default::default()
        };
        let parsed = Settings::from_json(&s.to_json().unwrap()).unwrap();
        assert_eq!(parsed.window, s.window);
    }

    #[test]
    fn from_json_missing_window_defaults_to_none() {
        let s = Settings::from_json(r#"{"version":1}"#).unwrap();
        assert_eq!(s.window, None);
    }

    #[test]
    fn normalize_floors_window_size() {
        // A hand-edited / undersized stored geometry is floored to the minimum,
        // mirroring how cache_size is normalized. Position is untouched.
        let mut s = Settings {
            window: Some(WindowGeometry {
                width: 100,
                height: 50,
                x: 7,
                y: 9,
            }),
            ..Default::default()
        };
        s.normalize();
        assert_eq!(
            s.window,
            Some(WindowGeometry {
                width: MIN_WINDOW_WIDTH,
                height: MIN_WINDOW_HEIGHT,
                x: 7,
                y: 9,
            })
        );
    }

    #[test]
    fn normalize_discards_inflated_window_geometry() {
        // The corruption that blanked the window: a scale-factor round-trip inflated the
        // stored size off-screen. Such geometry is discarded so the app boots at default.
        let mut s = Settings {
            window: Some(WindowGeometry {
                width: 110592,
                height: 1982,
                x: 0,
                y: 66,
            }),
            ..Default::default()
        };
        s.normalize();
        assert_eq!(s.window, None);
    }

    #[test]
    fn normalize_keeps_a_sane_window_geometry() {
        let geom = WindowGeometry {
            width: 1400,
            height: 900,
            x: 100,
            y: 100,
        };
        let mut s = Settings {
            window: Some(geom),
            ..Default::default()
        };
        s.normalize();
        assert_eq!(s.window, Some(geom));
    }

    #[test]
    fn from_json_preserves_preload_pages_zero() {
        let json = serde_json::json!({
            "version": SETTINGS_VERSION,
            "preload_pages": 0,
        })
        .to_string();
        let s = Settings::from_json(&json).unwrap();
        assert_eq!(s.preload_pages, 0, "preload_pages=0 must remain valid");
    }

    #[test]
    fn normalize_clamps_out_of_range_fields() {
        use crate::cache_config::{MAX_CACHE_SIZE, MAX_PREFETCH_RADIUS};
        // A `Settings` built any way other than `from_json` (e.g. hand-built in memory)
        // must still be brought inside its domain invariants by `normalize`.
        let mut s = Settings {
            cache_size: MAX_CACHE_SIZE + 50,
            preload_pages: MAX_PREFETCH_RADIUS + 10,
            recent_files: (0..MAX_RECENT_FILES + 5)
                .map(|i| PathBuf::from(format!("/p{i}")))
                .collect(),
            ..Settings::default()
        };
        s.normalize();
        assert_eq!(s.cache_size, MAX_CACHE_SIZE);
        assert_eq!(s.preload_pages, MAX_PREFETCH_RADIUS);
        assert_eq!(s.recent_files.len(), MAX_RECENT_FILES);
    }

    #[test]
    fn allow_rar_archives_missing_field_loads_as_true() {
        // A settings file written before this field existed must adopt the new
        // default (`true`) via `default_allow_rar`, not serde's bare `bool::default()`.
        let json = serde_json::json!({"version": SETTINGS_VERSION}).to_string();
        let s = Settings::from_json(&json).unwrap();
        assert!(
            s.allow_rar_archives,
            "missing allow_rar_archives must load as true (new default)"
        );
    }

    #[test]
    fn allow_rar_archives_true_round_trips() {
        let json = serde_json::json!({
            "version": SETTINGS_VERSION,
            "allow_rar_archives": true,
        })
        .to_string();
        let s = Settings::from_json(&json).unwrap();
        assert!(s.allow_rar_archives);
        let json2 = s.to_json().unwrap();
        let s2 = Settings::from_json(&json2).unwrap();
        assert!(s2.allow_rar_archives);
    }

    #[test]
    fn allow_rar_archives_explicit_false_round_trips() {
        // Now that the default is `true`, a user's explicit opt-out (`false`) must
        // survive load/save rather than being re-defaulted back to `true`.
        let json = serde_json::json!({
            "version": SETTINGS_VERSION,
            "allow_rar_archives": false,
        })
        .to_string();
        let s = Settings::from_json(&json).unwrap();
        assert!(!s.allow_rar_archives);
        let json2 = s.to_json().unwrap();
        let s2 = Settings::from_json(&json2).unwrap();
        assert!(!s2.allow_rar_archives);
    }

    #[test]
    fn update_fields_round_trip() {
        let s = Settings {
            auto_update_check: false,
            skipped_version: Some("0.12.0".to_string()),
            last_update_check: Some(1_712_345_678),
            ..Default::default()
        };
        let parsed = Settings::from_json(&s.to_json().unwrap()).unwrap();
        assert!(!parsed.auto_update_check);
        assert_eq!(parsed.skipped_version.as_deref(), Some("0.12.0"));
        assert_eq!(parsed.last_update_check, Some(1_712_345_678));
    }

    #[test]
    fn missing_auto_update_check_loads_as_true() {
        let s = Settings::from_json(r#"{"version":1}"#).unwrap();
        assert!(
            s.auto_update_check,
            "absent auto_update_check must load as true"
        );
    }

    #[test]
    fn none_skipped_and_last_check_are_omitted_from_json() {
        let json = Settings::default().to_json().unwrap();
        assert!(!json.contains("skipped_version"));
        assert!(!json.contains("last_update_check"));
    }
}
