//! Versioned JSON document persistence through the `BlobStore` port.
//!
//! `Settings::from_json` and `Library::from_json` both load a `{ "version": N, … }`
//! document: reject a non-object root, resolve the schema `version`, and run a
//! module-specific `migrate` only when the stored version is older than the
//! current one, while reporting a future version without deserializing it. That
//! guard is correctness-critical — a missed non-object check panics in `migrate`,
//! a truncating `as u32` cast would silently re-migrate a crafted huge version,
//! and deserializing a future document would drop unknown fields — so it lives
//! here once rather than in two aggregates that can drift. Each caller keeps its
//! own `from_value` mapping, error variant, and post-deserialize step; only the
//! shared prefix is single-homed.

use crate::{BlobStore, CoreError};
use std::{fmt, io, marker::PhantomData, sync::Arc};

pub(crate) trait JsonDocument: Sized {
    fn absent() -> Self;
    fn to_json(&self) -> Result<String, CoreError>;
    fn from_json(json: &str) -> Result<Self, CoreError>;
}

pub(crate) struct DocumentStore<D> {
    blobs: Arc<dyn BlobStore>,
    key: String,
    _doc: PhantomData<fn() -> D>,
}

impl<D> fmt::Debug for DocumentStore<D> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DocumentStore")
            .field("key", &self.key)
            .finish()
    }
}

impl<D: JsonDocument> DocumentStore<D> {
    pub(crate) fn new(blobs: Arc<dyn BlobStore>, key: impl Into<String>) -> Self {
        Self {
            blobs,
            key: key.into(),
            _doc: PhantomData,
        }
    }

    pub(crate) fn read_text(&self) -> Result<Option<String>, CoreError> {
        let Some(bytes) = self.blobs.read(&self.key)? else {
            return Ok(None);
        };
        let text = String::from_utf8(bytes).map_err(|_| {
            CoreError::Io(io::Error::new(
                io::ErrorKind::InvalidData,
                "stream did not contain valid UTF-8",
            ))
        })?;
        Ok(Some(text))
    }

    pub(crate) fn load(&self) -> Result<D, CoreError> {
        match self.read_text()? {
            Some(json) => D::from_json(&json),
            None => Ok(D::absent()),
        }
    }

    pub(crate) fn save(&self, doc: &D) -> Result<(), CoreError> {
        self.blobs
            .write_atomic(&self.key, doc.to_json()?.as_bytes())
    }

    pub(crate) fn quarantine(&self, now_secs: u64) -> Result<Option<String>, CoreError> {
        self.blobs.quarantine(&self.key, now_secs)
    }
}

/// Outcome of parsing a versioned-object document.
pub(crate) enum VersionedDocument {
    /// Stored version <= current: ready to deserialize (already migrated if older).
    Ready(serde_json::Value),
    /// Stored version > current: written by a NEWER build. This binary must not
    /// deserialize it (unknown fields would be dropped) nor save over it (the file
    /// would be downgraded while keeping its future label, permanently skipping
    /// every later migration — see F-C6). The caller turns this into its own
    /// `CoreError::FutureSchema`.
    FromFuture { found: u32 },
}

/// Parse a versioned-object JSON document: reject a non-object root, resolve the
/// schema `version` (truncating-cast-safe), migrate an older document, and report
/// a document from a future version without deserializing it.
pub(crate) fn parse_versioned_object(
    json: &str,
    current: u32,
    migrate: impl Fn(serde_json::Value, u32) -> serde_json::Value,
) -> Result<VersionedDocument, serde_json::Error> {
    let value: serde_json::Value = serde_json::from_str(json)?;
    if !value.is_object() {
        // Reject non-object roots (`5`, `[]`, `"x"`, …) that would panic `migrate`. Deserialize
        // into a Map (not `from_value::<T>`, whose `#[serde(default)]` fields would mask it).
        let err = serde_json::from_value::<serde_json::Map<String, serde_json::Value>>(value)
            .unwrap_err();
        return Err(err);
    }
    // Checked conversion, not a truncating `as u32`: a crafted `> u32::MAX` version is
    // treated as unknown (0) rather than wrapping small and triggering a bad migration.
    let from = value
        .get("version")
        .and_then(|v| v.as_u64())
        .and_then(|n| u32::try_from(n).ok())
        .unwrap_or(0);
    Ok(match from.cmp(&current) {
        std::cmp::Ordering::Less => VersionedDocument::Ready(migrate(value, from)),
        std::cmp::Ordering::Equal => VersionedDocument::Ready(value),
        std::cmp::Ordering::Greater => VersionedDocument::FromFuture { found: from },
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Library, MemoryBlobStore, Settings, LIBRARY_KEY, SETTINGS_KEY};

    const CURRENT: u32 = 2;

    #[test]
    fn document_store_absent_blob_loads_the_absent_document() {
        let blobs = Arc::new(MemoryBlobStore::new());
        let library = DocumentStore::<Library>::new(blobs.clone(), LIBRARY_KEY);
        let settings = DocumentStore::<Settings>::new(blobs, SETTINGS_KEY);

        assert_eq!(
            library.load().unwrap().to_json().unwrap(),
            Library::absent().to_json().unwrap(),
        );
        assert_eq!(settings.load().unwrap(), Settings::absent());
    }

    #[test]
    fn document_store_invalid_utf8_is_invalid_data() {
        let blobs = Arc::new(MemoryBlobStore::new());
        blobs.write_atomic(SETTINGS_KEY, &[0xff]).unwrap();
        let store = DocumentStore::<Settings>::new(blobs, SETTINGS_KEY);

        assert!(matches!(
            store.load(),
            Err(CoreError::Io(error)) if error.kind() == std::io::ErrorKind::InvalidData
        ));
    }

    /// A migrate stamp that records it ran by writing `migrated_from` into the doc
    /// and bumping `version` to CURRENT. Lets a test assert whether migration fired.
    fn stamp_migrate(mut value: serde_json::Value, from: u32) -> serde_json::Value {
        value["migrated_from"] = serde_json::json!(from);
        value["version"] = serde_json::json!(CURRENT);
        value
    }

    #[test]
    fn non_object_root_errors_without_panicking() {
        for src in ["5", "[]", "\"x\"", "true", "null"] {
            assert!(
                parse_versioned_object(src, CURRENT, stamp_migrate).is_err(),
                "expected Err for non-object root {src:?}"
            );
        }
    }

    #[test]
    fn invalid_json_propagates_the_parse_error() {
        assert!(parse_versioned_object("{ not json", CURRENT, stamp_migrate).is_err());
    }

    #[test]
    fn older_version_is_migrated() {
        let VersionedDocument::Ready(value) =
            parse_versioned_object(r#"{"version":0}"#, CURRENT, stamp_migrate).unwrap()
        else {
            panic!("older version must be ready after migration");
        };
        assert_eq!(value["migrated_from"], serde_json::json!(0));
        assert_eq!(value["version"], serde_json::json!(CURRENT));
    }

    #[test]
    fn missing_version_is_treated_as_zero_and_migrated() {
        let VersionedDocument::Ready(value) =
            parse_versioned_object(r#"{"books":[]}"#, CURRENT, stamp_migrate).unwrap()
        else {
            panic!("missing version must be ready after migration");
        };
        assert_eq!(value["migrated_from"], serde_json::json!(0));
    }

    #[test]
    fn huge_version_is_treated_as_unknown_and_migrated() {
        // 2^32 + 2 truncates to the current version (2) and would skip migration.
        let huge = u64::from(u32::MAX) + 3;
        let VersionedDocument::Ready(value) =
            parse_versioned_object(&format!(r#"{{"version":{huge}}}"#), CURRENT, stamp_migrate)
                .unwrap()
        else {
            panic!("huge version must be treated as unknown and migrated");
        };
        assert_eq!(value["migrated_from"], serde_json::json!(0));
    }

    #[test]
    fn current_version_is_returned_untouched() {
        let src = format!(r#"{{"version":{CURRENT},"keep":"me"}}"#);
        let VersionedDocument::Ready(value) =
            parse_versioned_object(&src, CURRENT, stamp_migrate).unwrap()
        else {
            panic!("current version must be ready");
        };
        // migrate must NOT have run: no stamp, original fields intact.
        assert!(value.get("migrated_from").is_none());
        assert_eq!(value["keep"], serde_json::json!("me"));
        assert_eq!(value["version"], serde_json::json!(CURRENT));
    }

    #[test]
    fn future_version_is_reported_and_not_migrated() {
        let migrated = std::cell::Cell::new(false);
        let result =
            parse_versioned_object(r#"{"version":3,"keep":"me"}"#, CURRENT, |value, from| {
                migrated.set(true);
                stamp_migrate(value, from)
            })
            .unwrap();

        assert!(matches!(result, VersionedDocument::FromFuture { found: 3 }));
        assert!(!migrated.get(), "future documents must not be migrated");
    }
}
