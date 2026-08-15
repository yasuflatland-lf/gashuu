//! Persistent settings storage.

use crate::{CoreError, Settings};
use directories::ProjectDirs;
use std::path::{Path, PathBuf};

/// Repository for `settings.json`: owns the file location and file I/O so
/// `Settings` remains a pure in-memory domain type. Construct with an explicit
/// path for tests or alternate locations, or resolve the OS config-dir default.
#[derive(Debug)]
pub struct SettingsStore {
    path: PathBuf,
}

impl SettingsStore {
    /// Construct a store for an explicit path.
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }

    /// Resolve `settings.json` in the OS config dir (creates nothing).
    pub fn default_location() -> Result<Self, CoreError> {
        let dirs = ProjectDirs::from("", "", "gashuu").ok_or(CoreError::NoConfigDir)?;
        Ok(Self::new(dirs.config_dir().join("settings.json")))
    }

    /// The file this store reads and writes.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Load settings. Missing files return defaults; other failures are errors.
    pub fn load(&self) -> Result<Settings, CoreError> {
        match std::fs::read_to_string(&self.path) {
            Ok(json) => Settings::from_json(&json),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Settings::default()),
            Err(e) => Err(CoreError::from(e)),
        }
    }

    /// Atomically write `settings`, creating parent directories as needed.
    pub fn save(&self, settings: &Settings) -> Result<(), CoreError> {
        crate::atomic_write::write_atomic(&self.path, settings.to_json()?.as_bytes())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::view_modes::{
        CoverMode, FitMode, KeyBindings, Language, ReadingDirection, SpreadMode,
    };
    use crate::window_geometry::WindowGeometry;
    use crate::SETTINGS_VERSION;

    fn non_default_settings() -> Settings {
        Settings {
            version: SETTINGS_VERSION,
            reading_direction: ReadingDirection::Ltr,
            spread_mode: SpreadMode::Double,
            cover_mode: CoverMode::Paired,
            fit_mode: FitMode::Whole,
            cache_capacity: 99,
            prefetch_radius: 4,
            key_bindings: KeyBindings {
                next: vec!["down".into()],
                prev: vec!["up".into()],
            },
            track_recent_sources: true,
            recent_sources: vec![PathBuf::from("/a"), PathBuf::from("/b")],
            language: Language::Ja,
            allow_rar_archives: false,
            window: Some(WindowGeometry {
                width: 1024,
                height: 768,
                x: 120,
                y: -40,
            }),
            auto_update_check: false,
            skipped_version: Some("v9.9.9".to_string()),
            last_update_check: Some(1_700_000_000),
        }
    }

    #[test]
    fn store_default_location_resolves_under_project_dirs() {
        let store = SettingsStore::default_location();

        assert!(store.is_ok());
        assert!(store
            .as_ref()
            .is_ok_and(|store| store.path().ends_with(Path::new("gashuu/settings.json"))));
    }

    #[test]
    fn save_then_load_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        // Path under a non-existent subdir to verify parent auto-creation.
        let path = dir.path().join("nested").join("sub").join("settings.json");
        let store = SettingsStore::new(path);
        let original = non_default_settings();
        store.save(&original).unwrap();
        let loaded = store.load().unwrap();
        assert_eq!(original, loaded);
    }

    #[test]
    fn save_creates_missing_parent_directories() {
        // The atomic helper owns parent creation; saving under a non-existent
        // subtree must materialize the directories AND the file.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("deep").join("nest").join("settings.json");
        assert!(!path.parent().unwrap().exists());
        SettingsStore::new(path.clone())
            .save(&Settings::default())
            .unwrap();
        assert!(path.exists());
    }

    #[test]
    fn save_overwrites_existing_file_with_complete_json() {
        // Saving over an existing (longer) settings file must replace it wholesale
        // with the new document — no truncation, no leftover tail from the old one.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        let store = SettingsStore::new(path.clone());

        // First save: a fully-populated (longer) document.
        store.save(&non_default_settings()).unwrap();

        // Second save: defaults (a shorter document on most fields).
        let replacement = Settings::default();
        store.save(&replacement).unwrap();

        // The bytes on disk must equal exactly the new serialization, and parse
        // back to the replacement value with no residue from the first write.
        let on_disk = std::fs::read_to_string(&path).unwrap();
        assert_eq!(on_disk, replacement.to_json().unwrap());
        assert_eq!(store.load().unwrap(), replacement);
    }

    #[test]
    fn missing_file_loads_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let store = SettingsStore::new(dir.path().join("does-not-exist.json"));
        let loaded = store.load().unwrap();
        assert_eq!(loaded, Settings::default());
    }

    #[test]
    fn corrupt_file_errors() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("corrupt.json");
        std::fs::write(&path, "not json").unwrap();
        let err = SettingsStore::new(path).load().unwrap_err();
        assert!(matches!(err, CoreError::Settings(_)));
    }

    #[test]
    fn load_normalizes_zero_cache_size_to_one() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        let json = serde_json::json!({
            "version": SETTINGS_VERSION,
            "cache_size": 0,
            "preload_pages": 0,
        })
        .to_string();
        std::fs::write(&path, json).unwrap();

        let s = SettingsStore::new(path).load().unwrap();

        assert_eq!(s.cache_capacity, 1, "cache_size=0 must be normalized to 1");
        assert_eq!(s.prefetch_radius, 0, "preload_pages=0 must NOT be clamped");
    }
}
