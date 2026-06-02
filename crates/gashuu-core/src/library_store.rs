//! Persistent library serialization.

use crate::error::CoreError;
use crate::library::Library;
use directories::ProjectDirs;
use std::path::{Path, PathBuf};

/// On-disk library schema version.
pub const LIBRARY_VERSION: u32 = 1;

impl Library {
    /// Resolve `library.json` in the OS data dir (creates nothing).
    pub fn data_path() -> Result<PathBuf, CoreError> {
        let dirs = ProjectDirs::from("", "", "gashuu").ok_or(CoreError::NoDataDir)?;
        Ok(dirs.data_dir().join("library.json"))
    }

    /// Load from the OS data path. Missing file returns an empty library.
    pub fn load() -> Result<Library, CoreError> {
        Self::load_from(&Self::data_path()?)
    }

    /// Load from an explicit path. Missing file returns an empty library.
    pub fn load_from(path: &Path) -> Result<Library, CoreError> {
        match std::fs::read_to_string(path) {
            Ok(json) => Self::from_json(&json),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Library::new()),
            Err(e) => Err(CoreError::from(e)),
        }
    }

    /// Save to the OS data path (creating parent dirs as needed).
    pub fn save(&self) -> Result<(), CoreError> {
        self.save_to(&Self::data_path()?)
    }

    pub fn save_to(&self, path: &Path) -> Result<(), CoreError> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, self.to_json()?)?;
        Ok(())
    }

    /// Serialize to pretty JSON with the on-disk schema version.
    pub fn to_json(&self) -> Result<String, CoreError> {
        let books = serde_json::to_value(self.books()).map_err(CoreError::Library)?;
        let value = serde_json::json!({
            "version": LIBRARY_VERSION,
            "books": books,
        });
        serde_json::to_string_pretty(&value).map_err(CoreError::Library)
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
        let mut library: Library = serde_json::from_value(value).map_err(CoreError::Library)?;
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::num::NonZeroUsize;
    use std::path::{Path, PathBuf};

    #[test]
    fn to_json_then_from_json_round_trips() {
        let mut lib = Library::new();
        let first = PathBuf::from("/manga/a.cbz");
        let second = PathBuf::from("/manga/b.cbz");
        assert!(lib.add(first.clone()).is_some());
        assert!(lib.add(second.clone()).is_some());
        assert!(lib.set_last_page(&second, 42));

        let json = lib.to_json().unwrap();
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
        assert!(lib.add(PathBuf::from("/manga/a.cbz")).is_some());

        let value: serde_json::Value = serde_json::from_str(&lib.to_json().unwrap()).unwrap();

        assert_eq!(value["version"].as_u64(), Some(u64::from(LIBRARY_VERSION)));
        assert!(value["books"].is_array());
        assert_eq!(value["books"][0]["path"], "/manga/a.cbz");
        assert_eq!(value["books"][0]["title"], "a");
        // Guard the schema: `page_count` must be emitted so it can't silently drop.
        assert_eq!(value["books"][0]["page_count"].as_u64(), Some(0));
    }

    #[test]
    fn from_json_empty_object_yields_empty_library() {
        let lib = Library::from_json("{}").unwrap();

        assert!(lib.books().is_empty());
    }

    #[test]
    fn from_json_non_object_root_errors() {
        for input in &["5", "[]", "\"x\"", "true", "null"] {
            let result = Library::from_json(input);
            assert!(
                matches!(result, Err(CoreError::Library(_))),
                "expected Err(CoreError::Library(_)) for input {:?}, got {:?}",
                input,
                result
            );
        }
    }

    #[test]
    fn from_json_corrupt_text_errors() {
        let result = Library::from_json("not json");

        assert!(
            matches!(result, Err(CoreError::Library(_))),
            "expected Err(CoreError::Library(_)), got {:?}",
            result
        );
    }

    #[test]
    fn from_json_huge_version_is_treated_as_unknown_and_migrates() {
        let json = serde_json::json!({
            "version": u64::from(u32::MAX) + 1,
        })
        .to_string();

        let lib = Library::from_json(&json).unwrap();

        assert!(lib.books().is_empty());
    }

    #[test]
    fn from_json_migrates_v0_to_current_version() {
        let value = serde_json::json!({"version": 0});
        let migrated = migrate(value, 0);

        assert_eq!(
            migrated["version"].as_u64(),
            Some(u64::from(LIBRARY_VERSION))
        );
        assert_eq!(migrated["books"].as_array().unwrap().len(), 0);
    }

    #[test]
    fn load_from_missing_file_returns_empty_library() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("does-not-exist.json");

        let lib = Library::load_from(&path).unwrap();

        assert!(lib.books().is_empty());
    }

    #[test]
    fn save_to_then_load_from_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested").join("library.json");
        let book = PathBuf::from("/manga/a.cbz");
        let mut original = Library::new();
        assert!(original.add(book.clone()).is_some());
        assert!(original.set_last_page(&book, 7));

        original.save_to(&path).unwrap();
        let loaded = Library::load_from(&path).unwrap();

        assert_eq!(loaded.books().len(), 1);
        assert_eq!(loaded.books()[0].path(), Path::new("/manga/a.cbz"));
        assert_eq!(loaded.books()[0].last_page(), 7);
    }

    #[test]
    fn load_from_normalizes_old_unsorted_library_order() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("library.json");
        let stored = r#"{"version":1,"books":[{"path":"/manga/vol 10.cbz","title":"vol 10","last_page":0,"page_count":0},{"path":"/manga/vol 1.cbz","title":"vol 1","last_page":0,"page_count":0},{"path":"/manga/vol 2.cbz","title":"vol 2","last_page":0,"page_count":0}]}"#;
        std::fs::write(&path, stored).unwrap();

        let loaded = Library::load_from(&path).unwrap();

        let titles: Vec<&str> = loaded.books().iter().map(|book| book.title()).collect();
        assert_eq!(titles, vec!["vol 1", "vol 2", "vol 10"]);
    }

    #[test]
    fn page_count_persists_across_save_and_load() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested").join("library.json");
        let book = PathBuf::from("/manga/a.cbz");
        let mut original = Library::new();
        assert!(original.add(book.clone()).is_some());
        assert!(original.set_page_count(&book, NonZeroUsize::new(42).unwrap()));

        original.save_to(&path).unwrap();
        let loaded = Library::load_from(&path).unwrap();

        assert_eq!(loaded.books().len(), 1);
        assert_eq!(loaded.books()[0].path(), Path::new("/manga/a.cbz"));
        assert_eq!(loaded.books()[0].page_count_opt(), Some(42));
    }

    #[test]
    fn load_from_corrupt_file_errors() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("corrupt.json");
        std::fs::write(&path, "not json").unwrap();

        let err = Library::load_from(&path).unwrap_err();

        assert!(matches!(err, CoreError::Library(_)));
    }

    #[test]
    fn data_path_targets_gashuu_library_json() {
        let path = Library::data_path().unwrap();

        assert!(path.ends_with("library.json"));
        assert!(path.to_string_lossy().contains("gashuu"));
    }

    /// Proves that `ReadingProgress` is transient: after serializing a library
    /// whose book has last_page and page_count set, the JSON object for that
    /// book contains EXACTLY the four documented keys and none of the
    /// progress-related keys that a future refactor might accidentally expose.
    #[test]
    fn reading_progress_is_not_persisted() {
        let mut lib = Library::new();
        let p = PathBuf::from("/manga/a.cbz");
        assert!(lib.add(p.clone()).is_some());
        assert!(lib.set_last_page(&p, 3));
        assert!(lib.set_page_count(&p, NonZeroUsize::new(10).unwrap()));

        let value: serde_json::Value = serde_json::from_str(&lib.to_json().unwrap()).unwrap();
        let book = value["books"][0].as_object().unwrap();

        assert_eq!(
            book.len(),
            4,
            "book serde shape must stay {{path,title,last_page,page_count}}"
        );
        for k in ["path", "title", "last_page", "page_count"] {
            assert!(book.contains_key(k), "missing expected key: {k}");
        }
        for k in [
            "progress",
            "reading_progress",
            "reached",
            "fraction",
            "current",
        ] {
            assert!(
                !book.contains_key(k),
                "ReadingProgress leaked into library.json: {k}"
            );
        }
    }

    /// Proves that an existing stored `library.json` still loads and
    /// re-serializes to the same structural shape after this PR (no format
    /// drift).  Comparison is order-independent because serde_json key
    /// ordering is not guaranteed.
    #[test]
    fn old_library_json_round_trips() {
        let stored = r#"{"version":1,"books":[{"path":"/manga/a.cbz","title":"a","last_page":42,"page_count":100}]}"#;
        let lib = Library::from_json(stored).unwrap();
        let reserialized = lib.to_json().unwrap();
        let before: serde_json::Value = serde_json::from_str(stored).unwrap();
        let after: serde_json::Value = serde_json::from_str(&reserialized).unwrap();
        assert_eq!(before, after, "library.json shape changed across load/save");
    }
}
