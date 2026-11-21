//! Byte-blob persistence for settings and library documents.

use crate::CoreError;
#[cfg(not(target_arch = "wasm32"))]
use directories::ProjectDirs;
use std::collections::HashMap;
#[cfg(not(target_arch = "wasm32"))]
use std::io;
#[cfg(not(target_arch = "wasm32"))]
use std::path::PathBuf;
use std::sync::Mutex;

pub const SETTINGS_KEY: &str = "settings.json";
pub const LIBRARY_KEY: &str = "library.json";

/// Keyed byte-blob persistence port (one document per key).
///
/// Keys are flat file-name-like strings.
pub trait BlobStore: Send + Sync + std::fmt::Debug {
    /// `Ok(None)` when `key` is absent.
    fn read(&self, key: &str) -> Result<Option<Vec<u8>>, CoreError>;

    /// Replace `key`'s bytes all-or-nothing (a reader never sees a partial document).
    fn write_atomic(&self, key: &str, bytes: &[u8]) -> Result<(), CoreError>;

    /// Move `key` aside as `<key>.corrupt-<now_secs>` without reading or parsing it.
    /// `Ok(Some(destination_label))` when moved, `Ok(None)` when `key` was absent.
    fn quarantine(&self, key: &str, now_secs: u64) -> Result<Option<String>, CoreError>;
}

/// In-memory adapter available on all targets.
#[derive(Debug, Default)]
pub struct MemoryBlobStore {
    blobs: Mutex<HashMap<String, Vec<u8>>>,
}

impl MemoryBlobStore {
    pub fn new() -> Self {
        Self::default()
    }
}

impl BlobStore for MemoryBlobStore {
    fn read(&self, key: &str) -> Result<Option<Vec<u8>>, CoreError> {
        let blobs = self.blobs.lock().expect("blob store mutex poisoned");
        Ok(blobs.get(key).cloned())
    }

    fn write_atomic(&self, key: &str, bytes: &[u8]) -> Result<(), CoreError> {
        let mut blobs = self.blobs.lock().expect("blob store mutex poisoned");
        blobs.insert(key.to_owned(), bytes.to_vec());
        Ok(())
    }

    fn quarantine(&self, key: &str, now_secs: u64) -> Result<Option<String>, CoreError> {
        let mut blobs = self.blobs.lock().expect("blob store mutex poisoned");
        let Some(bytes) = blobs.remove(key) else {
            return Ok(None);
        };
        let destination = format!("{key}.corrupt-{now_secs}");
        blobs.insert(destination.clone(), bytes);
        Ok(Some(destination))
    }
}

/// Filesystem adapter rooted at one directory.
#[cfg(not(target_arch = "wasm32"))]
#[derive(Debug, Clone)]
pub struct FsBlobStore {
    dir: PathBuf,
}

#[cfg(not(target_arch = "wasm32"))]
impl FsBlobStore {
    pub fn new(dir: PathBuf) -> Self {
        Self { dir }
    }

    pub fn config_dir() -> Result<Self, CoreError> {
        let dirs = ProjectDirs::from("", "", "gashuu").ok_or(CoreError::NoConfigDir)?;
        Ok(Self::new(dirs.config_dir().to_path_buf()))
    }

    pub fn data_dir() -> Result<Self, CoreError> {
        let dirs = ProjectDirs::from("", "", "gashuu").ok_or(CoreError::NoDataDir)?;
        Ok(Self::new(dirs.data_dir().to_path_buf()))
    }

    pub fn path_of(&self, key: &str) -> PathBuf {
        self.dir.join(key)
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl BlobStore for FsBlobStore {
    fn read(&self, key: &str) -> Result<Option<Vec<u8>>, CoreError> {
        match std::fs::read(self.path_of(key)) {
            Ok(bytes) => Ok(Some(bytes)),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error.into()),
        }
    }

    fn write_atomic(&self, key: &str, bytes: &[u8]) -> Result<(), CoreError> {
        crate::atomic_write::write_atomic(&self.path_of(key), bytes)
    }

    fn quarantine(&self, key: &str, now_secs: u64) -> Result<Option<String>, CoreError> {
        match crate::persist::quarantine_file(&self.path_of(key), now_secs) {
            Ok(destination) => Ok(Some(
                destination
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into_owned(),
            )),
            Err(CoreError::Io(error)) if error.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Library, LibraryStore, Settings, SettingsStore};
    use std::path::PathBuf;
    use std::sync::Arc;

    fn contract(store: &dyn BlobStore) {
        let key = SETTINGS_KEY;
        let now_secs = 1_700_000_000;
        let destination = "settings.json.corrupt-1700000000";

        assert_eq!(store.read(key).unwrap(), None);
        assert_eq!(store.quarantine(key, now_secs).unwrap(), None);
        assert_eq!(store.read(key).unwrap(), None);
        assert_eq!(store.read(destination).unwrap(), None);

        store.write_atomic(key, b"original bytes").unwrap();
        assert_eq!(store.read(key).unwrap(), Some(b"original bytes".to_vec()));

        store.write_atomic(key, b"new bytes").unwrap();
        assert_eq!(store.read(key).unwrap(), Some(b"new bytes".to_vec()));

        assert_eq!(
            store.quarantine(key, now_secs).unwrap().as_deref(),
            Some(destination)
        );
        assert_eq!(store.read(key).unwrap(), None);
        let moved_bytes = store.read(destination).unwrap();
        assert_eq!(moved_bytes, Some(b"new bytes".to_vec()));
    }

    #[test]
    fn memory_blob_store_contract() {
        contract(&MemoryBlobStore::new());
    }

    #[test]
    fn settings_store_load_missing_is_default() {
        let store = SettingsStore::with_blob_store(Arc::new(MemoryBlobStore::new()));
        assert_eq!(store.load().unwrap(), Settings::default());
    }

    #[test]
    fn library_store_load_missing_is_empty() {
        let store = LibraryStore::with_blob_store(Arc::new(MemoryBlobStore::new()));
        let library = store.load().unwrap();
        assert!(library.books().is_empty());
        assert!(library.last_opened().is_none());
    }

    #[test]
    fn stores_round_trip_through_memory_store() {
        let blobs = Arc::new(MemoryBlobStore::new());
        let settings_store = SettingsStore::with_blob_store(blobs.clone());
        let library_store = LibraryStore::with_blob_store(blobs.clone());
        let settings = Settings {
            track_recent_sources: true,
            ..Settings::default()
        };
        let mut library = Library::new();
        let book = PathBuf::from("/manga/a.cbz");
        assert!(library.add(book.clone()).is_some());
        assert!(library.set_resume_page(&book, 2));

        settings_store.save(&settings).unwrap();
        library_store.save(&library).unwrap();

        assert_eq!(
            blobs.read(SETTINGS_KEY).unwrap(),
            Some(settings.to_json().unwrap().into_bytes())
        );
        assert_eq!(
            blobs.read(LIBRARY_KEY).unwrap(),
            Some(library.to_json().unwrap().into_bytes())
        );
        assert_eq!(settings_store.load().unwrap(), settings);
        let loaded = library_store.load().unwrap();
        assert_eq!(loaded.books().len(), 1);
        assert_eq!(loaded.books()[0].path(), book.as_path());
        assert_eq!(loaded.books()[0].resume_page(), 2);
    }

    #[test]
    fn non_utf8_blob_errors_with_invalid_data() {
        let blobs = Arc::new(MemoryBlobStore::new());
        blobs.write_atomic(SETTINGS_KEY, b"\xff").unwrap();
        blobs.write_atomic(LIBRARY_KEY, b"\xff").unwrap();
        let settings_store = SettingsStore::with_blob_store(blobs.clone());
        let library_store = LibraryStore::with_blob_store(blobs);

        for error in [
            settings_store.load().unwrap_err(),
            settings_store.stored_text().unwrap_err(),
            library_store.load().unwrap_err(),
        ] {
            let CoreError::Io(error) = error else {
                panic!("expected I/O error, got {error:?}");
            };
            assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);
            assert_eq!(error.to_string(), "stream did not contain valid UTF-8");
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    mod fs_tests {
        use super::*;

        #[test]
        fn fs_blob_store_contract() {
            let dir = tempfile::tempdir().unwrap();
            contract(&FsBlobStore::new(dir.path().to_path_buf()));
        }

        #[test]
        fn write_atomic_creates_missing_dir() {
            let root = tempfile::tempdir().unwrap();
            let dir = root.path().join("nested").join("store");
            let store = FsBlobStore::new(dir.clone());

            assert!(!dir.exists());
            store.write_atomic(SETTINGS_KEY, b"settings").unwrap();
            assert_eq!(std::fs::read(dir.join(SETTINGS_KEY)).unwrap(), b"settings");
        }

        #[test]
        fn path_of_joins_dir_and_key() {
            let dir = PathBuf::from("root").join("store");
            let store = FsBlobStore::new(dir.clone());
            assert_eq!(store.path_of(SETTINGS_KEY), dir.join("settings.json"));
        }

        #[test]
        fn config_dir_and_data_dir_resolve_under_project_dirs() {
            let config = FsBlobStore::config_dir().unwrap();
            let data = FsBlobStore::data_dir().unwrap();

            let settings_path = config.path_of(SETTINGS_KEY);
            let library_path = data.path_of(LIBRARY_KEY);
            assert!(settings_path.ends_with("gashuu/settings.json"));
            assert!(library_path.ends_with("gashuu/library.json"));
        }
    }
}
