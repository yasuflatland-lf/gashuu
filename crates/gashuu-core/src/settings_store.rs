//! Persistent settings storage.

use crate::persist::{DocumentStore, JsonDocument};
#[cfg(not(target_arch = "wasm32"))]
use crate::FsBlobStore;
use crate::{BlobStore, CoreError, Settings, SETTINGS_KEY};
#[cfg(not(target_arch = "wasm32"))]
use std::path::PathBuf;
use std::sync::Arc;

/// Repository for a settings document stored as a JSON blob.
#[derive(Debug)]
pub struct SettingsStore {
    inner: DocumentStore<Settings>,
}

impl SettingsStore {
    /// Use the settings key in the supplied blob store.
    pub fn with_blob_store(blobs: Arc<dyn BlobStore>) -> Self {
        Self {
            inner: DocumentStore::new(blobs, SETTINGS_KEY),
        }
    }

    /// Construct a store for an explicit path.
    ///
    /// # Panics
    ///
    /// The path must end in a UTF-8 file name.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn new(path: PathBuf) -> Self {
        let (blobs, key) = FsBlobStore::for_file(&path);
        Self {
            inner: DocumentStore::new(Arc::new(blobs), key),
        }
    }

    /// Resolve `settings.json` in the OS config dir (creates nothing).
    #[cfg(not(target_arch = "wasm32"))]
    pub fn default_location() -> Result<Self, CoreError> {
        Ok(Self::with_blob_store(Arc::new(FsBlobStore::config_dir()?)))
    }

    /// Load settings. An absent blob returns defaults.
    pub fn load(&self) -> Result<Settings, CoreError> {
        self.inner.load()
    }

    /// Atomically write the settings document.
    pub fn save(&self, settings: &Settings) -> Result<(), CoreError> {
        self.inner.save(settings)
    }

    /// Move the settings blob aside and return its destination label if present.
    pub fn quarantine(&self, now_secs: u64) -> Result<Option<String>, CoreError> {
        self.inner.quarantine(now_secs)
    }

    /// Raw stored text for startup repair. An absent blob returns `Ok(None)`.
    /// Invalid UTF-8 returns an I/O error of kind `InvalidData`.
    pub fn stored_text(&self) -> Result<Option<String>, CoreError> {
        self.inner.read_text()
    }
}

impl JsonDocument for Settings {
    fn absent() -> Self {
        Self::default()
    }

    fn to_json(&self) -> Result<String, CoreError> {
        Settings::to_json(self)
    }

    fn from_json(json: &str) -> Result<Self, CoreError> {
        Settings::from_json(json)
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
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
            web_guidance_dismissed: ["evicted".to_owned()].into_iter().collect(),
        }
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
    fn load_normalizes_zero_cache_size_to_the_floor() {
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

        assert_eq!(s.cache_capacity, 2);
        assert_eq!(s.prefetch_radius, 0, "preload_pages=0 must NOT be clamped");
    }
}
