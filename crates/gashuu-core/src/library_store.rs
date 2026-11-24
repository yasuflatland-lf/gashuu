//! Persistent library serialization.

use crate::error::CoreError;
use crate::library::{Book, Library};
use crate::persist::{DocumentStore, JsonDocument};
#[cfg(not(target_arch = "wasm32"))]
use crate::FsBlobStore;
use crate::{BlobStore, BookId, LIBRARY_KEY};
use serde::Serialize;
#[cfg(all(any(test, feature = "testing"), not(target_arch = "wasm32")))]
use std::path::PathBuf;
use std::sync::Arc;

/// On-disk library schema version.
pub const LIBRARY_VERSION: u32 = 1;

/// The on-disk library document. Owns the complete serialized schema in one
/// place so a new top-level field can no longer be silently omitted: every
/// field's omission rule (e.g. `last_opened`'s `skip_serializing_if`) is the
/// derived `Serialize` behavior `to_json` actually uses.
#[derive(Serialize)]
struct LibraryDocument<'a> {
    version: u32,
    books: &'a [Book],
    #[serde(skip_serializing_if = "Option::is_none")]
    last_opened: Option<&'a BookId>,
}

/// Repository for a library document stored as a JSON blob.
#[derive(Debug)]
pub struct LibraryStore {
    inner: DocumentStore<Library>,
}

impl LibraryStore {
    /// Use the library key in the supplied blob store.
    pub fn with_blob_store(blobs: Arc<dyn BlobStore>) -> Self {
        Self {
            inner: DocumentStore::new(blobs, LIBRARY_KEY),
        }
    }

    /// Test-support constructor for a store at an explicit path.
    ///
    /// # Panics
    ///
    /// The path must end in a UTF-8 file name.
    #[cfg(all(any(test, feature = "testing"), not(target_arch = "wasm32")))]
    pub fn new(path: PathBuf) -> Self {
        let (blobs, key) = FsBlobStore::for_file(&path);
        Self {
            inner: DocumentStore::new(Arc::new(blobs), key),
        }
    }

    /// Resolve `library.json` in the OS data dir (creates nothing).
    #[cfg(not(target_arch = "wasm32"))]
    pub fn default_location() -> Result<Self, CoreError> {
        Ok(Self::with_blob_store(Arc::new(FsBlobStore::data_dir()?)))
    }

    /// Load the library. An absent blob returns an empty library.
    pub fn load(&self) -> Result<Library, CoreError> {
        self.inner.load()
    }

    /// Atomically write the library document.
    pub fn save(&self, library: &Library) -> Result<(), CoreError> {
        self.inner.save(library)
    }

    /// Move the library blob aside and return its destination label if present.
    pub fn quarantine(&self, now_secs: u64) -> Result<Option<String>, CoreError> {
        self.inner.quarantine(now_secs)
    }
}

impl JsonDocument for Library {
    fn absent() -> Self {
        Self::new()
    }

    fn to_json(&self) -> Result<String, CoreError> {
        Library::to_json(self)
    }

    fn from_json(json: &str) -> Result<Self, CoreError> {
        Library::from_json(json)
    }
}

impl Library {
    /// Serialize to pretty JSON with the on-disk schema version.
    ///
    /// Serialization goes through a single derived `Serialize` path
    /// ([`LibraryDocument`]): the top-level shape and every field's omission
    /// rule (e.g. `last_opened`'s `skip_serializing_if`) are owned by that one
    /// type, so a new top-level field cannot be silently dropped on save.
    pub fn to_json(&self) -> Result<String, CoreError> {
        let document = LibraryDocument {
            version: LIBRARY_VERSION,
            books: self.books(),
            last_opened: self.last_opened(),
        };
        serde_json::to_string_pretty(&document).map_err(CoreError::Library)
    }

    /// Parse JSON, migrating older schema versions to the current shape.
    pub fn from_json(json: &str) -> Result<Self, CoreError> {
        // Non-object guard + version resolution + migrate dispatch are single-homed in
        // `persist`. `CoreError::Library` is deliberately not `#[from]` (see error.rs).
        let value = match crate::persist::parse_versioned_object(json, LIBRARY_VERSION, migrate)
            .map_err(CoreError::Library)?
        {
            crate::persist::VersionedDocument::Ready(value) => value,
            crate::persist::VersionedDocument::FromFuture { found } => {
                return Err(CoreError::FutureSchema {
                    what: "library",
                    found,
                    supported: LIBRARY_VERSION,
                });
            }
        };
        let mut library: Library = serde_json::from_value(value).map_err(CoreError::Library)?;
        // Repair legacy desktop paths before target-independent shelf normalization.
        #[cfg(not(target_arch = "wasm32"))]
        library.recanonicalize_and_merge();
        library.normalize();
        Ok(library)
    }
}

/// Upgrade a raw library JSON value from `from` to the current schema version.
fn migrate(mut value: serde_json::Value, from: u32) -> serde_json::Value {
    let mut version = from;
    if version == 0 {
        if value.get("books").is_none() {
            value["books"] = serde_json::json!([]);
        }
        version = 1;
    }
    value["version"] = serde_json::json!(version);
    value
}

#[cfg(all(test, not(target_arch = "wasm32")))]
#[path = "library_store/tests.rs"]
mod tests;
