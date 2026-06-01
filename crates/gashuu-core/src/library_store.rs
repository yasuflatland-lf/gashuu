//! Persistent library serialization.

use crate::error::CoreError;
use crate::library::Library;

/// On-disk library schema version.
pub const LIBRARY_VERSION: u32 = 1;

impl Library {
    /// Serialize to pretty JSON with the on-disk schema version.
    pub fn to_json(&self) -> String {
        let books = serde_json::to_value(self.books()).unwrap_or(serde_json::Value::Null);
        let value = serde_json::json!({
            "version": LIBRARY_VERSION,
            "books": books,
        });
        serde_json::to_string_pretty(&value).unwrap_or_else(|_| "{}".to_string())
    }

    /// Parse JSON, migrating older schema versions to the current shape.
    pub fn from_json(json: &str) -> Result<Self, CoreError> {
        let value: serde_json::Value = serde_json::from_str(json).map_err(CoreError::Library)?;
        if !value.is_object() {
            let err = serde_json::from_value::<serde_json::Map<String, serde_json::Value>>(value)
                .unwrap_err();
            return Err(CoreError::Library(err));
        }
        let from = value
            .get("version")
            .and_then(|v| v.as_u64())
            .and_then(|n| u32::try_from(n).ok())
            .unwrap_or(0);
        let value = if from < LIBRARY_VERSION {
            migrate(value, from)
        } else {
            value
        };
        serde_json::from_value(value).map_err(CoreError::Library)
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::{Path, PathBuf};

    #[test]
    fn to_json_then_from_json_round_trips() {
        let mut lib = Library::new();
        let first = PathBuf::from("/manga/a.cbz");
        let second = PathBuf::from("/manga/b.cbz");
        assert!(lib.add(first.clone()));
        assert!(lib.add(second.clone()));
        assert!(lib.set_last_page(&second, 42));

        let json = lib.to_json();
        let parsed = Library::from_json(&json).unwrap();

        assert_eq!(parsed.books().len(), 2);
        assert_eq!(parsed.books()[0].path(), Path::new("/manga/a.cbz"));
        assert_eq!(parsed.books()[0].title(), "a");
        assert_eq!(parsed.books()[0].last_page(), 0);
        assert_eq!(parsed.books()[1].path(), Path::new("/manga/b.cbz"));
        assert_eq!(parsed.books()[1].title(), "b");
        assert_eq!(parsed.books()[1].last_page(), 42);
    }

    #[test]
    fn to_json_emits_version_and_books() {
        let mut lib = Library::new();
        assert!(lib.add(PathBuf::from("/manga/a.cbz")));

        let value: serde_json::Value = serde_json::from_str(&lib.to_json()).unwrap();

        assert_eq!(value["version"].as_u64(), Some(u64::from(LIBRARY_VERSION)));
        assert!(value["books"].is_array());
        assert_eq!(value["books"][0]["path"], "/manga/a.cbz");
        assert_eq!(value["books"][0]["title"], "a");
    }

    #[test]
    fn from_json_empty_object_yields_empty_library() {
        let lib = Library::from_json("{}").unwrap();

        assert!(lib.books().is_empty());
    }
}
