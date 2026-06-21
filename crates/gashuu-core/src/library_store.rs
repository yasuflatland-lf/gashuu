//! Persistent library serialization.

use crate::error::CoreError;
use crate::library::{Book, Library};
use directories::ProjectDirs;
use serde::Serialize;
use std::path::{Path, PathBuf};

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
    last_opened: Option<&'a Path>,
}

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

    /// Atomically write the library to `path`, creating parent directories as needed.
    pub fn save_to(&self, path: &Path) -> Result<(), CoreError> {
        crate::atomic_write::write_atomic(path, self.to_json()?.as_bytes())
    }

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
    fn to_json_emits_positive_page_count_as_bare_integer() {
        // Pins the `Some(n)` direction of the `Option<NonZeroUsize>` serde shim:
        // a known count must serialize as the bare integer `n` (not an object or
        // a tagged enum), keeping the on-disk shape byte-compatible. The `None`
        // direction (emitted as `0`) is pinned by `to_json_emits_version_and_books`.
        let mut lib = Library::new();
        let book = PathBuf::from("/manga/a.cbz");
        assert!(lib.add(book.clone()).is_some());
        assert!(lib.set_page_count(&book, NonZeroUsize::new(123).unwrap()));

        let value: serde_json::Value = serde_json::from_str(&lib.to_json().unwrap()).unwrap();

        assert_eq!(value["books"][0]["page_count"].as_u64(), Some(123));
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
    fn save_to_creates_missing_parent_directories() {
        // The atomic helper owns parent creation; saving under a non-existent
        // subtree must materialize the directories AND the file.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("deep").join("nest").join("library.json");
        assert!(!path.parent().unwrap().exists());
        Library::new().save_to(&path).unwrap();
        assert!(path.exists());
    }

    #[test]
    fn save_to_overwrites_existing_file_with_complete_json() {
        // Saving over an existing (longer) library file must replace it wholesale
        // with the new document — no truncation, no leftover tail from the old one.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("library.json");

        // First save: two books (a longer document).
        let mut original = Library::new();
        assert!(original.add(PathBuf::from("/manga/a.cbz")).is_some());
        assert!(original.add(PathBuf::from("/manga/b.cbz")).is_some());
        original.save_to(&path).unwrap();

        // Second save: a single empty library (a shorter document).
        let replacement = Library::new();
        replacement.save_to(&path).unwrap();

        // The bytes on disk must equal exactly the new serialization, with no
        // residue from the first write, and parse back to an empty library.
        let on_disk = std::fs::read_to_string(&path).unwrap();
        assert_eq!(on_disk, replacement.to_json().unwrap());
        assert!(Library::load_from(&path).unwrap().books().is_empty());
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

    #[test]
    fn book_without_overrides_loads_as_empty() {
        // A library.json written before this feature has no "overrides" key; it
        // must load with an all-None (inherit-global) override.
        let stored = r#"{"version":1,"books":[{"path":"/manga/a.cbz","title":"a","last_page":5,"page_count":100}]}"#;
        let lib = Library::from_json(stored).unwrap();
        assert!(lib.books()[0].overrides().is_empty());
    }

    #[test]
    fn empty_overrides_are_omitted_from_json() {
        // skip_serializing_if: a book with no overrides must NOT emit an
        // "overrides" key, so the on-disk shape is unchanged for existing books.
        let mut lib = Library::new();
        assert!(lib.add(PathBuf::from("/manga/a.cbz")).is_some());
        let value: serde_json::Value = serde_json::from_str(&lib.to_json().unwrap()).unwrap();
        assert!(value["books"][0].get("overrides").is_none());
    }

    #[test]
    fn non_empty_overrides_round_trip() {
        let mut lib = Library::new();
        let p = PathBuf::from("/manga/a.cbz");
        assert!(lib.add(p.clone()).is_some());
        let ov = crate::view_override::ViewOverride {
            reading_direction: Some(crate::view_modes::ReadingDirection::Rtl),
            spread_mode: Some(crate::view_modes::SpreadMode::Double),
            cover_mode: Some(crate::view_modes::CoverMode::Paired),
            fit_mode: Some(crate::view_modes::FitMode::Width),
        };
        assert!(lib.set_overrides(&p, ov));
        let reloaded = Library::from_json(&lib.to_json().unwrap()).unwrap();
        assert_eq!(reloaded.books()[0].overrides(), ov);
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

        // A book with EMPTY overrides serializes to exactly {path,title,last_page,page_count};
        // the "overrides" key is omitted via skip_serializing_if (see empty_overrides_are_omitted_from_json).
        // A non-empty override adds an "overrides" key (exercised by non_empty_overrides_round_trip).
        assert_eq!(
            book.len(),
            4,
            "book with empty overrides must have exactly 4 keys: {{path,title,last_page,page_count}}"
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

    // --- last_opened persistence tests ---

    #[test]
    fn old_json_without_last_opened_loads_as_none() {
        // Pre-feature library.json has no "last_opened" key; must load as None.
        let stored = r#"{"version":1,"books":[{"path":"/manga/a.cbz","title":"a","last_page":0,"page_count":0}]}"#;
        let lib = Library::from_json(stored).unwrap();
        assert_eq!(lib.last_opened(), None);
    }

    #[test]
    fn old_json_without_last_opened_omits_key_on_resave() {
        // A library loaded from old JSON (no last_opened) must not emit the key
        // on re-save: `skip_serializing_if = "Option::is_none"` on the derived
        // LibraryDocument path (the single serialization path) omits it.
        let stored = r#"{"version":1,"books":[{"path":"/manga/a.cbz","title":"a","last_page":42,"page_count":100}]}"#;
        let lib = Library::from_json(stored).unwrap();
        let value: serde_json::Value = serde_json::from_str(&lib.to_json().unwrap()).unwrap();
        assert!(
            value.get("last_opened").is_none(),
            "last_opened must be absent when None"
        );
    }

    #[test]
    fn to_json_omits_last_opened_when_none_emits_when_some() {
        // Pin the single serialization path: `last_opened` is omitted when None
        // and present with the exact stored path when Some.
        let none_value: serde_json::Value =
            serde_json::from_str(&Library::new().to_json().unwrap()).unwrap();
        assert!(
            none_value.get("last_opened").is_none(),
            "last_opened must be absent when None"
        );

        let book = PathBuf::from("/manga/a.cbz");
        let mut lib = Library::new();
        lib.register_opened(&book, None);
        let some_value: serde_json::Value = serde_json::from_str(&lib.to_json().unwrap()).unwrap();
        assert_eq!(
            some_value.get("last_opened").and_then(|v| v.as_str()),
            Some("/manga/a.cbz"),
            "last_opened must be emitted with the stored path when Some"
        );
    }

    #[test]
    fn last_opened_round_trips_through_save_and_load() {
        // A library with last_opened set keeps the value across to_json/from_json.
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("library.json");
        let book = PathBuf::from("/manga/a.cbz");
        let mut lib = Library::new();
        lib.register_opened(&book, NonZeroUsize::new(10));

        lib.save_to(&file_path).unwrap();
        let loaded = Library::load_from(&file_path).unwrap();

        assert_eq!(
            loaded.last_opened(),
            Some(Path::new("/manga/a.cbz")),
            "last_opened must survive save/load"
        );
    }

    #[test]
    fn save_to_preserves_last_opened() {
        // Verify that last_opened survives a full save_to → load_from round-trip.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("library.json");
        let book = PathBuf::from("/manga/vol1.cbz");
        let mut lib = Library::new();
        assert!(lib.add(book.clone()).is_some());
        lib.register_opened(&book, None);
        // last_opened is set before saving.
        assert_eq!(lib.last_opened(), Some(Path::new("/manga/vol1.cbz")));

        lib.save_to(&path).unwrap();
        let loaded = Library::load_from(&path).unwrap();

        assert_eq!(
            loaded.last_opened(),
            Some(Path::new("/manga/vol1.cbz")),
            "last_opened must survive save_to/load_from"
        );
    }

    #[test]
    fn last_opened_orphan_is_normalized_to_none_on_load() {
        // A library.json whose last_opened path is NOT in books must normalize to None.
        let stored = serde_json::json!({
            "version": 1,
            "books": [{"path": "/manga/a.cbz", "title": "a", "last_page": 0, "page_count": 0}],
            "last_opened": "/manga/gone.cbz"
        })
        .to_string();
        let lib = Library::from_json(&stored).unwrap();
        assert_eq!(
            lib.last_opened(),
            None,
            "orphan last_opened must be normalized to None on load"
        );
    }

    #[test]
    fn last_opened_to_json_then_from_json_round_trips() {
        // In-memory round-trip via to_json/from_json (without touching the FS).
        let book = PathBuf::from("/manga/a.cbz");
        let mut lib = Library::new();
        lib.register_opened(&book, None);
        let json = lib.to_json().unwrap();
        let parsed = Library::from_json(&json).unwrap();
        assert_eq!(
            parsed.last_opened(),
            Some(Path::new("/manga/a.cbz")),
            "last_opened must survive to_json/from_json"
        );
    }
}
