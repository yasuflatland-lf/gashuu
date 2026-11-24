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

    mod props {
        use super::*;
        use proptest::prelude::*;
        use serde_json::{json, Value};
        use std::cell::Cell;

        fn version_input() -> impl Strategy<Value = (u32, Option<Value>)> {
            (1u32..=5).prop_flat_map(|current| {
                let u32_max = u64::from(u32::MAX);
                let unsigned = prop_oneof![
                    0..=u64::from(current) + 3,
                    u32_max - 1..=u32_max + 1,
                    Just(u32_max + 3),
                    Just(u64::MAX),
                    any::<u64>(),
                ];
                let version = prop_oneof![
                    Just(None),
                    prop::sample::select(vec![json!(-1), json!(1.5), json!("2"), json!(null)])
                        .prop_map(Some),
                    unsigned.prop_map(|version| Some(json!(version))),
                ];
                (Just(current), version)
            })
        }

        proptest! {
            /// C2-27: Versioned objects migrate only when their effective version is older.
            /// Replaces `non_object_root_errors_without_panicking`, `older_version_is_migrated`,
            /// `missing_version_is_treated_as_zero_and_migrated`,
            /// `huge_version_is_treated_as_unknown_and_migrated`,
            /// `current_version_is_returned_untouched`, and
            /// `future_version_is_reported_and_not_migrated`.
            #[test]
            fn prop_versioned_object_routes_by_effective_version(
                (current, version) in version_input(),
                payload in any::<u32>(),
            ) {
                let mut original = json!({"payload": payload});
                let effective = version.as_ref()
                    .and_then(Value::as_u64)
                    .map_or(0, |version| u32::try_from(version).unwrap_or(0));
                if let Some(version) = version {
                    original["version"] = version;
                }

                let calls = Cell::new(0);
                let parsed = parse_versioned_object(&original.to_string(), current, |mut value, from| {
                    calls.set(calls.get() + 1);
                    value["migrated_from"] = json!(from);
                    value["version"] = json!(current);
                    value
                }).unwrap();
                let actual = match parsed {
                    VersionedDocument::Ready(value) => Ok(value),
                    VersionedDocument::FromFuture { found } => Err(found),
                };

                let mut expected = original;
                if effective < current {
                    expected["migrated_from"] = json!(effective);
                    expected["version"] = json!(current);
                }
                let expected = if effective > current { Err(effective) } else { Ok(expected) };
                prop_assert_eq!(actual, expected);
                prop_assert_eq!(calls.get(), u32::from(effective < current));

                for root in [json!(payload), json!([payload]), json!(payload.to_string()), json!(true), Value::Null] {
                    let result = parse_versioned_object(&root.to_string(), current, |_, _| {
                        panic!("a non-object must never reach migration");
                    });
                    prop_assert!(result.is_err(), "expected a parse error for {root}");
                }
            }
        }
    }

    #[test]
    fn invalid_json_propagates_the_parse_error() {
        assert!(parse_versioned_object("{ not json", 2, |value, _| value).is_err());
    }
}
