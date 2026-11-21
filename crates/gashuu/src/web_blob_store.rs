//! Browser-backed persistence for the settings and library JSON documents.

use gashuu_core::{BlobStore, CoreError};
use std::io;
#[cfg(target_arch = "wasm32")]
use web_sys::wasm_bindgen::JsValue;

pub(crate) const STORAGE_PREFIX: &str = "gashuu/";

/// Minimal string key-value seam over browser storage or a test map.
pub(crate) trait KeyValue: Send + Sync + std::fmt::Debug {
    fn get(&self, name: &str) -> Result<Option<String>, CoreError>;
    fn set(&self, name: &str, value: &str) -> Result<(), CoreError>;
    fn remove(&self, name: &str) -> Result<(), CoreError>;
}

/// A blob store for UTF-8 documents under the app's storage prefix.
#[derive(Debug)]
pub(crate) struct KvBlobStore<K: KeyValue> {
    kv: K,
}

impl<K: KeyValue> KvBlobStore<K> {
    pub(crate) fn new(kv: K) -> Self {
        Self { kv }
    }
}

impl<K: KeyValue> BlobStore for KvBlobStore<K> {
    fn read(&self, key: &str) -> Result<Option<Vec<u8>>, CoreError> {
        self.kv
            .get(&format!("{STORAGE_PREFIX}{key}"))
            .map(|value| value.map(String::into_bytes))
    }

    fn write_atomic(&self, key: &str, bytes: &[u8]) -> Result<(), CoreError> {
        let value = std::str::from_utf8(bytes)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        self.kv.set(&format!("{STORAGE_PREFIX}{key}"), value)
    }

    fn quarantine(&self, key: &str, now_secs: u64) -> Result<Option<String>, CoreError> {
        let name = format!("{STORAGE_PREFIX}{key}");
        let Some(value) = self.kv.get(&name)? else {
            return Ok(None);
        };
        let destination = format!("{key}.corrupt-{now_secs}");
        self.kv
            .set(&format!("{STORAGE_PREFIX}{destination}"), &value)?;
        self.kv.remove(&name)?;
        Ok(Some(destination))
    }
}

/// Reacquires the browser's storage handle on each operation.
#[cfg(target_arch = "wasm32")]
#[derive(Debug)]
pub(crate) struct LocalStorageKv;

#[cfg(target_arch = "wasm32")]
impl LocalStorageKv {
    /// Returns a backend when localStorage is currently reachable.
    pub(crate) fn detect() -> Option<Self> {
        Self::storage().ok().map(|_| Self)
    }

    fn storage() -> Result<web_sys::Storage, JsValue> {
        web_sys::window()
            .ok_or(JsValue::UNDEFINED)?
            .local_storage()?
            .ok_or(JsValue::NULL)
    }
}

#[cfg(target_arch = "wasm32")]
impl KeyValue for LocalStorageKv {
    fn get(&self, name: &str) -> Result<Option<String>, CoreError> {
        Self::storage()
            .and_then(|storage| storage.get_item(name))
            .map_err(|js| storage_error("getItem", js))
    }

    fn set(&self, name: &str, value: &str) -> Result<(), CoreError> {
        Self::storage()
            .and_then(|storage| storage.set_item(name, value))
            .map_err(|js| storage_error("setItem", js))
    }

    fn remove(&self, name: &str) -> Result<(), CoreError> {
        Self::storage()
            .and_then(|storage| storage.remove_item(name))
            .map_err(|js| storage_error("removeItem", js))
    }
}

#[cfg(target_arch = "wasm32")]
fn storage_error(op: &str, js: JsValue) -> CoreError {
    CoreError::Io(io::Error::other(format!("localStorage {op}: {js:?}")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use gashuu_core::{Library, LibraryStore, Settings, SettingsStore};
    use std::collections::HashMap;
    use std::path::PathBuf;
    use std::sync::{Arc, Mutex};

    #[derive(Debug, Default)]
    struct MemoryKv(Mutex<HashMap<String, String>>);

    impl KeyValue for MemoryKv {
        fn get(&self, name: &str) -> Result<Option<String>, CoreError> {
            Ok(self.0.lock().unwrap().get(name).cloned())
        }

        fn set(&self, name: &str, value: &str) -> Result<(), CoreError> {
            self.0
                .lock()
                .unwrap()
                .insert(name.to_owned(), value.to_owned());
            Ok(())
        }

        fn remove(&self, name: &str) -> Result<(), CoreError> {
            self.0.lock().unwrap().remove(name);
            Ok(())
        }
    }

    #[derive(Debug)]
    struct FailingKv;

    impl KeyValue for FailingKv {
        fn get(&self, _name: &str) -> Result<Option<String>, CoreError> {
            Err(io::Error::other("get failed").into())
        }

        fn set(&self, _name: &str, _value: &str) -> Result<(), CoreError> {
            Err(io::Error::other("set failed").into())
        }

        fn remove(&self, _name: &str) -> Result<(), CoreError> {
            Err(io::Error::other("remove failed").into())
        }
    }

    #[test]
    fn read_missing_key_is_none() {
        let store = KvBlobStore::new(MemoryKv::default());

        assert_eq!(store.read("settings.json").unwrap(), None);
    }

    #[test]
    fn write_then_read_round_trips_bytes() {
        let store = KvBlobStore::new(MemoryKv::default());
        let json = "{\"title\":\"caf\u{e9}\"}";

        store
            .write_atomic("settings.json", json.as_bytes())
            .unwrap();

        assert_eq!(
            store.read("settings.json").unwrap().as_deref(),
            Some(json.as_bytes())
        );
        assert_eq!(
            store.kv.get("gashuu/settings.json").unwrap().as_deref(),
            Some(json)
        );
    }

    #[test]
    fn write_rejects_non_utf8() {
        let kv = MemoryKv::default();
        kv.set("gashuu/settings.json", "{}").unwrap();
        let store = KvBlobStore::new(kv);

        let result = store.write_atomic("settings.json", &[0xff]);

        assert!(matches!(result, Err(CoreError::Io(e)) if e.kind() == io::ErrorKind::InvalidData));
        assert_eq!(
            *store.kv.0.lock().unwrap(),
            HashMap::from([("gashuu/settings.json".to_owned(), "{}".to_owned())])
        );
    }

    #[test]
    fn quarantine_moves_value_and_returns_unprefixed_name() {
        let kv = MemoryKv::default();
        kv.set("gashuu/library.json", "garbage").unwrap();
        let store = KvBlobStore::new(kv);

        let destination = store.quarantine("library.json", 1_700_000_000).unwrap();

        assert_eq!(
            destination.as_deref(),
            Some("library.json.corrupt-1700000000")
        );
        assert_eq!(
            *store.kv.0.lock().unwrap(),
            HashMap::from([(
                "gashuu/library.json.corrupt-1700000000".to_owned(),
                "garbage".to_owned()
            )])
        );
    }

    #[test]
    fn quarantine_missing_key_is_none_and_writes_nothing() {
        let store = KvBlobStore::new(MemoryKv::default());

        assert_eq!(
            store.quarantine("library.json", 1_700_000_000).unwrap(),
            None
        );
        assert!(store.kv.0.lock().unwrap().is_empty());
    }

    #[test]
    fn kv_errors_propagate() {
        let store = KvBlobStore::new(FailingKv);
        let read = store.read("settings.json").unwrap_err();
        let write = store.write_atomic("settings.json", b"{}").unwrap_err();
        let quarantine = store
            .quarantine("settings.json", 1_700_000_000)
            .unwrap_err();

        for (error, message) in [
            (read, "get failed"),
            (write, "set failed"),
            (quarantine, "get failed"),
        ] {
            assert!(matches!(error, CoreError::Io(e) if e.to_string() == message));
        }
    }

    #[test]
    fn settings_and_library_round_trip_through_kv_blob_store() {
        let blobs: Arc<dyn BlobStore> = Arc::new(KvBlobStore::new(MemoryKv::default()));
        let settings_store = SettingsStore::with_blob_store(Arc::clone(&blobs));
        let library_store = LibraryStore::with_blob_store(blobs);
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

        assert_eq!(settings_store.load().unwrap(), settings);
        assert_eq!(
            library_store.load().unwrap().to_json().unwrap(),
            library.to_json().unwrap()
        );
    }
}
