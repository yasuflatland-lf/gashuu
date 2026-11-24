use super::*;
use proptest::prelude::*;
use std::num::NonZeroUsize;
use std::path::Path;

proptest! {
    #[test]
    fn library_json_round_trips_byte_identically(lib in Library::shelves_for_persistence_tests()) {
        let json = lib.to_json().unwrap();

        prop_assert_eq!(Library::from_json(&json).unwrap().to_json().unwrap(), json);
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(32))]

    #[test]
    fn library_store_round_trips_byte_identically(lib in Library::shelves_for_persistence_tests()) {
        let dir = tempfile::tempdir().unwrap();
        let json = lib.to_json().unwrap();
        let path = dir.path().join("library.json");
        let store = LibraryStore::new(path.clone());

        store.save(&lib).unwrap();

        prop_assert_eq!(std::fs::read_to_string(path).unwrap(), json.as_str());
        prop_assert_eq!(store.load().unwrap().to_json().unwrap(), json);
    }
}

fn add(lib: &mut Library, raw: &str) -> BookId {
    let id = BookId::new(raw);
    let title = title(&id);
    lib.add(id, title).unwrap().clone()
}

fn title(id: &BookId) -> String {
    crate::display_title(Path::new(id.as_str()), false)
}

#[test]
fn library_json_is_byte_identical_across_bookid_change() {
    let fixture = r#"{
  "version": 1,
  "books": [
    {
      "path": "/manga/Series/Vol 1.cbz",
      "title": "Vol 1",
      "last_page": 4,
      "page_count": 12,
      "overrides": {
        "reading_direction": "rtl"
      }
    },
    {
      "path": "/manga/Vol 2.cbz",
      "title": "Vol 2",
      "last_page": 0,
      "page_count": 0
    }
  ],
  "last_opened": "/manga/Series/Vol 1.cbz"
}"#;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("library.json");
    std::fs::write(&path, fixture).unwrap();
    let store = LibraryStore::new(path.clone());

    assert_eq!(
        Library::from_json(fixture).unwrap().to_json().unwrap(),
        fixture
    );
    let lib = store.load().unwrap();
    store.save(&lib).unwrap();
    assert_eq!(std::fs::read_to_string(path).unwrap(), fixture);
}

#[test]
fn to_json_emits_version_and_books() {
    let mut lib = Library::new();
    add(&mut lib, "/manga/a.cbz");

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
    // Pins the `Some(n)` direction of the serde shim: a known count serializes as the
    // bare integer `n`, keeping the on-disk shape byte-compatible (`None` emits as `0`).
    let mut lib = Library::new();
    let book = add(&mut lib, "/manga/a.cbz");
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
    // 2^32 + 2 truncates to 2, which would be rejected as a future version.
    let json = serde_json::json!({
        "version": u64::from(u32::MAX) + 3,
    })
    .to_string();

    let result = Library::from_json(&json);

    assert!(
        matches!(&result, Ok(lib) if lib.books().is_empty()),
        "{result:?}"
    );
}

#[test]
fn from_json_future_version_errors_without_downgrading() {
    let result = Library::from_json(r#"{"version":2,"books":[]}"#);

    assert!(matches!(
        result,
        Err(CoreError::FutureSchema {
            what: "library",
            found: 2,
            supported: 1,
        })
    ));
}

#[test]
fn load_from_future_version_file_leaves_the_file_untouched() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("library.json");
    let original = br#"{"version":2,"books":[],"future_field":"keep me"}"#;
    std::fs::write(&path, original).unwrap();

    let result = LibraryStore::new(path.clone()).load();

    assert!(matches!(
        result,
        Err(CoreError::FutureSchema {
            what: "library",
            found: 2,
            supported: 1,
        })
    ));
    assert_eq!(std::fs::read(&path).unwrap(), original);
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

    let lib = LibraryStore::new(path).load().unwrap();

    assert!(lib.books().is_empty());
}

#[test]
fn save_to_creates_missing_parent_directories() {
    // The atomic helper owns parent creation; saving under a non-existent
    // subtree must materialize the directories AND the file.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("deep").join("nest").join("library.json");
    assert!(!path.parent().unwrap().exists());
    LibraryStore::new(path.clone())
        .save(&Library::new())
        .unwrap();
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
    add(&mut original, "/manga/a.cbz");
    add(&mut original, "/manga/b.cbz");
    let store = LibraryStore::new(path.clone());
    store.save(&original).unwrap();

    // Second save: a single empty library (a shorter document).
    let replacement = Library::new();
    store.save(&replacement).unwrap();

    // The bytes on disk must equal exactly the new serialization, with no
    // residue from the first write, and parse back to an empty library.
    let on_disk = std::fs::read_to_string(&path).unwrap();
    assert_eq!(on_disk, replacement.to_json().unwrap());
    assert!(store.load().unwrap().books().is_empty());
}

#[test]
fn load_from_normalizes_old_unsorted_library_order() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("library.json");
    let stored = r#"{"version":1,"books":[{"path":"/manga/vol 10.cbz","title":"vol 10","last_page":0,"page_count":0},{"path":"/manga/vol 1.cbz","title":"vol 1","last_page":0,"page_count":0},{"path":"/manga/vol 2.cbz","title":"vol 2","last_page":0,"page_count":0}]}"#;
    std::fs::write(&path, stored).unwrap();

    let loaded = LibraryStore::new(path).load().unwrap();

    let titles: Vec<&str> = loaded.books().iter().map(|book| book.title()).collect();
    assert_eq!(titles, vec!["vol 1", "vol 2", "vol 10"]);
}

#[test]
fn load_from_corrupt_file_errors() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("corrupt.json");
    std::fs::write(&path, "not json").unwrap();

    let err = LibraryStore::new(path).load().unwrap_err();

    assert!(matches!(err, CoreError::Library(_)));
}

#[test]
fn library_store_quarantine_renames_aside() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("library.json");
    std::fs::write(&path, "not json").unwrap();
    let expected = dir.path().join("library.json.corrupt-1700000000");

    let destination = LibraryStore::new(path.clone())
        .quarantine(1_700_000_000)
        .unwrap();

    assert_eq!(destination, Some("library.json.corrupt-1700000000".into()));
    assert!(!path.exists());
    assert_eq!(std::fs::read(&expected).unwrap(), b"not json");
}

#[test]
fn quarantine_corrupt_file_returns_none_for_missing_source_without_creating_destination() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("library.json");
    let destination = dir.path().join("library.json.corrupt-1700000000");

    let result = LibraryStore::new(path).quarantine(1_700_000_000).unwrap();
    assert_eq!(result, None);
    assert!(!destination.exists());
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
    add(&mut lib, "/manga/a.cbz");
    let value: serde_json::Value = serde_json::from_str(&lib.to_json().unwrap()).unwrap();
    assert!(value["books"][0].get("overrides").is_none());
}

/// Proves that `ReadingProgress` is transient: after serializing a library
/// whose book has resume_page and page_count set, the JSON object for that
/// book contains EXACTLY the four documented keys and none of the
/// progress-related keys that a future refactor might accidentally expose.
#[test]
fn reading_progress_is_not_persisted() {
    let mut lib = Library::new();
    let p = add(&mut lib, "/manga/a.cbz");
    assert!(lib.set_resume_page(&p, 3));
    assert!(lib.set_page_count(&p, NonZeroUsize::new(10).unwrap()));

    let value: serde_json::Value = serde_json::from_str(&lib.to_json().unwrap()).unwrap();
    let book = value["books"][0].as_object().unwrap();

    // A book with EMPTY overrides serializes to exactly {path,title,last_page,page_count};
    // the "overrides" key is omitted via skip_serializing_if (a non-empty one adds it).
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
        "last_viewed",
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
    // A library loaded from old JSON (no last_opened) must not emit the key on re-save:
    // `skip_serializing_if = "Option::is_none"` on the single serialize path omits it.
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

    let book = BookId::new("/manga/a.cbz");
    let mut lib = Library::new();
    lib.register_opened(&book, &title(&book), None);
    let some_value: serde_json::Value = serde_json::from_str(&lib.to_json().unwrap()).unwrap();
    assert_eq!(
        some_value.get("last_opened").and_then(|v| v.as_str()),
        Some("/manga/a.cbz"),
        "last_opened must be emitted with the stored path when Some"
    );
}

#[test]
fn recanonicalize_round_trips_clean_library_byte_identically() {
    // Byte-oracle: an already-canonical, duplicate-free library must survive the
    // load-time re-canonicalize/merge byte-for-byte. Override + position keep it non-vacuous.
    let dir = tempfile::tempdir().unwrap();
    let mut lib = Library::new();
    for name in ["a.cbz", "b.cbz"] {
        let f = dir.path().join(name);
        std::fs::write(&f, []).unwrap();
        assert!(lib
            .add(
                BookId::from_canonical_path(&crate::canonical_identity(&f)).unwrap(),
                crate::display_title(&f, false)
            )
            .is_some());
    }
    let canonical_b =
        BookId::from_canonical_path(&dir.path().join("b.cbz").canonicalize().unwrap()).unwrap();
    assert!(lib.set_resume_page(&canonical_b, 4));
    assert!(lib.set_page_count(&canonical_b, NonZeroUsize::new(12).unwrap()));
    let ov = crate::view_override::ViewOverride {
        reading_direction: Some(crate::view_modes::ReadingDirection::Rtl),
        ..crate::view_override::ViewOverride::none()
    };
    assert!(lib.set_overrides(&canonical_b, ov));
    lib.register_opened(&canonical_b, &title(&canonical_b), None);

    let before = lib.to_json().unwrap();
    let reloaded = Library::from_json(&before).unwrap();

    assert_eq!(
        reloaded.to_json().unwrap(),
        before,
        "a clean, all-canonical library round-trips byte-identically"
    );
}
