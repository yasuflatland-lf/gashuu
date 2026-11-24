//! Persistent user settings domain model and JSON representation.

use crate::archive_loader::ArchivePolicy;
use crate::cache::{DEFAULT_CAPACITY, DEFAULT_PREFETCH_RADIUS};
use crate::cache_config::CacheConfig;
use crate::error::CoreError;
use crate::view_modes::{CoverMode, FitMode, KeyBindings, Language, ReadingDirection, SpreadMode};
use crate::view_override::ResolvedView;
use crate::window_geometry::WindowGeometry;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::path::PathBuf;

/// On-disk schema version. Bump when the shape changes and add a `migrate` step.
pub const SETTINGS_VERSION: u32 = 1;
/// Maximum number of recently opened sources (files or folders) retained.
pub const MAX_RECENT_SOURCES: usize = 10;

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
    #[serde(default = "default_cache_capacity", rename = "cache_size")]
    pub cache_capacity: usize,
    #[serde(default = "default_prefetch_radius", rename = "preload_pages")]
    pub prefetch_radius: usize,
    #[serde(default)]
    pub key_bindings: KeyBindings,
    /// Record recently opened sources (files or folders). Off by default
    /// (privacy); `recent_sources` is only updated when this is true. The on-disk
    /// key stays `track_recent_files` for byte-compatibility (`serde(rename)`).
    #[serde(default, rename = "track_recent_files")]
    pub track_recent_sources: bool,
    /// Recently opened sources (files or folders), most-recent first. Capped at
    /// `MAX_RECENT_SOURCES`. The on-disk key stays `recent_files` for
    /// byte-compatibility (`serde(rename)`).
    #[serde(default, rename = "recent_files")]
    pub recent_sources: Vec<PathBuf>,
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
    /// Browser storage guidance kinds dismissed by the user.
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub web_guidance_dismissed: BTreeSet<String>,
}

fn default_version() -> u32 {
    SETTINGS_VERSION
}
fn default_cache_capacity() -> usize {
    DEFAULT_CAPACITY
}
fn default_prefetch_radius() -> usize {
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
            cache_capacity: DEFAULT_CAPACITY,
            prefetch_radius: DEFAULT_PREFETCH_RADIUS,
            key_bindings: KeyBindings::default(),
            track_recent_sources: false,
            recent_sources: Vec::new(),
            language: Language::default(),
            allow_rar_archives: true,
            window: None,
            auto_update_check: true,
            skipped_version: None,
            last_update_check: None,
            web_guidance_dismissed: BTreeSet::new(),
        }
    }
}

impl Settings {
    /// The global view-mode defaults a per-book `ViewOverride` inherits from.
    pub fn view_defaults(&self) -> ResolvedView {
        ResolvedView {
            reading_direction: self.reading_direction,
            spread_mode: self.spread_mode,
            cover_mode: self.cover_mode,
            fit_mode: self.fit_mode,
        }
    }

    /// Replace all four global view defaults with a resolved view.
    pub fn set_view_defaults(&mut self, view: ResolvedView) {
        self.reading_direction = view.reading_direction;
        self.spread_mode = view.spread_mode;
        self.cover_mode = view.cover_mode;
        self.fit_mode = view.fit_mode;
    }

    /// Derive the archive-open policy from these settings.
    pub fn archive_policy(&self) -> ArchivePolicy {
        ArchivePolicy {
            allow_rar: self.allow_rar_archives,
        }
    }

    /// Serialize to pretty JSON (also used by the snapshot test).
    pub fn to_json(&self) -> Result<String, CoreError> {
        serde_json::to_string_pretty(self).map_err(CoreError::Settings)
    }

    /// Parse JSON, migrating older schema versions to the current shape.
    pub fn from_json(json: &str) -> Result<Self, CoreError> {
        // Non-object guard + version resolution + migrate dispatch are single-homed in
        // `persist`.
        let value = match crate::persist::parse_versioned_object(json, SETTINGS_VERSION, migrate)
            .map_err(CoreError::Settings)?
        {
            crate::persist::VersionedDocument::Ready(value) => value,
            crate::persist::VersionedDocument::FromFuture { found } => {
                return Err(CoreError::FutureSchema {
                    what: "settings",
                    found,
                    supported: SETTINGS_VERSION,
                });
            }
        };
        let mut settings: Self = serde_json::from_value(value).map_err(CoreError::Settings)?;
        settings.normalize();
        Ok(settings)
    }

    /// Re-establish domain invariants on a freshly built or loaded `Settings`,
    /// so that a value constructed any way other than the normal write path
    /// (a hand-edited file, a future loader, an in-memory value) still obeys
    /// the same bounds. Mirrors `Library::normalize`. Idempotent.
    ///
    /// Persist the cache capacity and radius after applying the bounds defined
    /// by [`CacheConfig::new`].
    pub fn normalize(&mut self) {
        // The `version` field is a SCHEMA LABEL owned by the binary, not user data: a value
        // loaded from disk is never echoed back on save (F-C6 / CORE-S-16). Loads can only
        // reach here with a version <= SETTINGS_VERSION (a higher one is now a hard error),
        // and migrate already stamps the older ones, so this is the belt-and-braces guard
        // for in-memory values built outside the load path. Idempotent.
        self.version = SETTINGS_VERSION;
        self.store_cache_config(self.cache_config());
        // push_recent caps recent_sources on write, but a hand-edited file could exceed
        // MAX_RECENT_SOURCES and persist forever (exit-save writes in-memory state); cap here.
        self.recent_sources.truncate(MAX_RECENT_SOURCES);
        self.window = self.window.and_then(WindowGeometry::sanitized);
    }

    /// Record `path` as most-recently-opened when tracking is enabled. Dedups
    /// (moves an existing entry to the front), caps at `MAX_RECENT_SOURCES`. No-op
    /// when `track_recent_sources` is false.
    pub fn push_recent(&mut self, path: PathBuf) {
        if !self.track_recent_sources {
            return;
        }
        self.recent_sources.retain(|p| p != &path);
        self.recent_sources.insert(0, path);
        self.recent_sources.truncate(MAX_RECENT_SOURCES);
    }

    /// The canonical conversion from persisted cache settings to a validated
    /// configuration. [`CacheConfig::new`] defines the bounds and capacity floor.
    pub fn cache_config(&self) -> CacheConfig {
        CacheConfig::new(self.cache_capacity, self.prefetch_radius)
    }

    /// Validate the requested capacity with the stored radius and persist both values.
    pub fn set_cache_capacity(&mut self, capacity: usize) -> CacheConfig {
        self.store_cache_config(CacheConfig::new(capacity, self.prefetch_radius))
    }

    /// Validate the requested radius and persist both values, raising the stored
    /// capacity when needed to fit the prefetch window.
    pub fn set_prefetch_radius(&mut self, radius: usize) -> CacheConfig {
        self.store_cache_config(CacheConfig::new(self.cache_capacity, radius))
    }

    fn store_cache_config(&mut self, cfg: CacheConfig) -> CacheConfig {
        self.cache_capacity = cfg.capacity();
        self.prefetch_radius = cfg.radius();
        cfg
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
#[path = "settings/tests.rs"]
mod tests;
