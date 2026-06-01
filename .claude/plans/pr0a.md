# PR-0a: Library Model + library.json + Thumbnail Cache Skeleton — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add the headless `gashuu-core` foundation for a persistent multi-book library: `Book`/`Library` domain types, a versioned `library.json` store mirroring `settings.rs` discipline, and a compile-only `ThumbnailCache` skeleton (with a pure stable `cache_key`) that PR-T will fill in.

**Architecture:** Three new modules live in `gashuu-core` and stay headless (no `slint`, no `tracing`): `library.rs` holds `Book` (identity = canonical path, derived title, last-page) and `Library` (ordered, dedup-by-path `Vec<Book>` with insertion-order carousel semantics); `library_store.rs` adds JSON (de)serialization + OS-path/explicit-path load/save mirroring `Settings`; `thumbnail_cache.rs` ships a `ThumbnailCache` struct + `new`/`with_dir` + a pure `cache_key` while `get`/`put` are deliberate no-ops (skeleton) so downstream PRs link against the frozen API. `error.rs` gains two non-breaking `CoreError` variants and `lib.rs` re-exports the new public surface.

**Tech Stack:** `serde` + `serde_json` (de/serialize + non-object-root guard + `u32::try_from` version parse), `directories::ProjectDirs` (OS config/cache dirs), `std::hash::{Hash, Hasher}` + `std::collections::hash_map::DefaultHasher` (stable hex `cache_key`, no new dependency), `thiserror` (`CoreError`), `tempfile` (dev-dep, file round-trip tests). `image`'s `DecodedImage` is referenced only in the skeleton signatures.

---

## File Structure

| File | Status | Single responsibility |
| --- | --- | --- |
| `crates/gashuu-core/src/library.rs` | Create | `Book` + `Library` domain types and invariants (dedup, insertion order, last-page, derived availability). Headless, no I/O beyond `canonicalize`/`exists`. |
| `crates/gashuu-core/src/library_store.rs` | Create | `impl Library` JSON + persistence: `from_json`/`to_json`, `load_from`/`save_to`, OS `load`/`save`, `LIBRARY_VERSION`, migration hook. Mirrors `settings.rs`. |
| `crates/gashuu-core/src/thumbnail_cache.rs` | Create | `ThumbnailCache` struct + `new`/`with_dir` + pure `cache_key`; `get`→`None` / `put`→no-op SKELETON (bodies land in PR-T). |
| `crates/gashuu-core/src/error.rs` | Modify | Add `CoreError::Library(serde_json::Error)` (NO `#[from]`) + `CoreError::NoDataDir`. |
| `crates/gashuu-core/src/lib.rs` | Modify | `mod` declarations + public re-exports of the new types/consts/fns. |

**Frozen-contract conformance notes (read before writing code):**
- `Book` fields are PRIVATE; the JSON shape is `{ "path", "title", "last_page" }`. Serialize/deserialize via serde with explicit struct fields (private fields still (de)serialize). `title` is derived at `add` time and stored (round-trips); availability is NEVER stored.
- `Library` private field is `books: Vec<Book>`; JSON shape `{ "version": 1, "books": [...] }`.
- `from_json` MUST guard a non-object root BEFORE any migrate (same gotcha as `Settings`), and parse `version` via `u32::try_from` (never `as u32`).
- `CoreError` already has `Settings(#[from] serde_json::Error)`. You CANNOT add a second `#[from] serde_json::Error`. Add `Library(serde_json::Error)` WITHOUT `#[from]` and construct via `.map_err(CoreError::Library)`. `CoreError` is `#[non_exhaustive]`, so this is non-breaking.
- `cache_key` is PURE (no I/O): caller passes the already-resolved `mtime_secs`. Use `DefaultHasher` over `(path-as-bytes, mtime_secs, max_side)` → lowercase hex of the `u64` → filename stem. Stable within a process/std version; that is sufficient (a changed std hash just regenerates the on-disk cache, which is mtime-keyed anyway and lives in the OS cache dir).
- `ThumbnailCache::get`/`put` are SKELETON: `get` returns `None`, `put` does nothing and returns `Ok(())`. No `todo!`. They must COMPILE and reference `crate::image_ops::DecodedImage` per the frozen signatures so PR-T only fills bodies.

---

## Task 1 — `error.rs`: add `Library` (no `#[from]`) + `NoDataDir` variants

**Files:**
- Modify: `crates/gashuu-core/src/error.rs`
- Test: `crates/gashuu-core/src/error.rs` (inline `#[cfg(test)] mod tests`)

- [ ] **1.1 Write failing tests.** Append these two tests inside the existing `#[cfg(test)] mod tests { ... }` block in `crates/gashuu-core/src/error.rs` (after `unsupported_format_displays_path`):
```rust
    #[test]
    fn library_displays_with_prefix() {
        let json_err = serde_json::from_str::<serde_json::Value>("not json").unwrap_err();
        let err = CoreError::Library(json_err);
        assert!(err.to_string().starts_with("library format error: "));
    }

    #[test]
    fn no_data_dir_displays_message() {
        let err = CoreError::NoDataDir;
        assert_eq!(err.to_string(), "no data directory available for library");
    }
```
- [ ] **1.2 Run it (expect FAIL):** `mise exec -- cargo nextest run -p gashuu-core library_displays_with_prefix no_data_dir_displays_message`
  - Expected FAIL: compile error `no variant or associated item named 'Library'`/`'NoDataDir' found for enum 'CoreError'` — the variants do not exist yet.
- [ ] **1.3 Minimal impl.** In `crates/gashuu-core/src/error.rs`, add the two variants to the `CoreError` enum. Insert them immediately AFTER the existing `NoConfigDir` variant (keeps the settings/library pairing adjacent):
```rust
    /// Library file could not be (de)serialized. NOTE: deliberately NOT
    /// `#[from] serde_json::Error` — `CoreError::Settings` already owns that
    /// `From` impl, and a type can have only one. Construct explicitly via
    /// `.map_err(CoreError::Library)`.
    #[error("library format error: {0}")]
    Library(serde_json::Error),

    /// The OS did not provide a data directory for library storage.
    #[error("no data directory available for library")]
    NoDataDir,
```
- [ ] **1.4 Run it (expect PASS):** `mise exec -- cargo nextest run -p gashuu-core library_displays_with_prefix no_data_dir_displays_message`
  - Expected PASS: both tests pass; the crate compiles (no second `#[from] serde_json::Error`).
- [x] **1.5 Commit:** `git commit -m "feat(core): add CoreError::Library and CoreError::NoDataDir variants"`

Progress: DONE 2026-06-02 - CoreError::Library/NoDataDir added; focused nextest passed.

---

## Task 2 — `library.rs`: `Book` type + `lib.rs` wiring

**Files:**
- Create: `crates/gashuu-core/src/library.rs`
- Modify: `crates/gashuu-core/src/lib.rs`
- Test: `crates/gashuu-core/src/library.rs` (inline tests)

- [x] **2.1 Write failing test.** Create `crates/gashuu-core/src/library.rs` with ONLY the test module first so it fails to compile against the not-yet-written `Book`:
```rust
//! Library domain model: an ordered, de-duplicated shelf of books.
//!
//! Headless (no slint, no tracing). Identity is the canonical filesystem path;
//! availability is derived (never stored). Persistence lives in `library_store`.

use std::path::{Path, PathBuf};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn book_derives_title_from_file_stem() {
        let book = Book::from_path(PathBuf::from("/manga/Cool Title.cbz"));
        assert_eq!(book.title(), "Cool Title");
        assert_eq!(book.path(), Path::new("/manga/Cool Title.cbz"));
        assert_eq!(book.last_page(), 0);
    }

    #[test]
    fn book_derives_title_from_dir_name_when_no_extension() {
        // A folder book has no extension; the title is the directory name.
        let book = Book::from_path(PathBuf::from("/manga/My Folder Book"));
        assert_eq!(book.title(), "My Folder Book");
    }

    #[test]
    fn book_title_falls_back_to_path_string_when_no_stem() {
        // Pathological path with no file_stem (e.g. root) falls back to the lossy
        // path string so the title is never empty.
        let book = Book::from_path(PathBuf::from("/"));
        assert!(!book.title().is_empty());
    }
}
```
- [x] **2.2 Run it (expect FAIL):** `mise exec -- cargo nextest run -p gashuu-core book_derives_title_from_file_stem`
  - Expected FAIL: `module 'library' not found` / `cannot find function 'Book' / type 'Book'` — `library.rs` is not yet declared in `lib.rs` and `Book` does not exist.
- [x] **2.3 Minimal impl (part A: declare the module).** In `crates/gashuu-core/src/lib.rs`, add a `pub mod library;` declaration. Insert it alphabetically after `pub mod image_ops;`:
```rust
pub mod library;
```
- [x] **2.4 Minimal impl (part B: write `Book`).** Insert this above the `#[cfg(test)] mod tests` block in `crates/gashuu-core/src/library.rs`:
```rust
use serde::{Deserialize, Serialize};

/// One book in the shelf. Identity is the canonical filesystem path.
///
/// Carries display data only: the path, a derived `title` (file stem for a file,
/// directory name for a folder), and the leading page index of the last-viewed
/// spread. The book *kind* (folder / archive) is resolved at open time by
/// `ArchiveLoader` and is deliberately NOT persisted. Availability (whether the
/// path still resolves) is derived at render time, never stored.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Book {
    path: PathBuf,
    title: String,
    #[serde(default)]
    last_page: usize,
}

impl Book {
    /// Build a `Book` from a path, deriving the display title. The title is the
    /// file stem (e.g. `Cool Title` from `Cool Title.cbz`) or the directory name
    /// for an extension-less folder, falling back to the lossy full path string
    /// so the title is never empty.
    pub(crate) fn from_path(path: PathBuf) -> Self {
        let title = path
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| path.to_string_lossy().into_owned());
        Self {
            path,
            title,
            last_page: 0,
        }
    }

    /// The canonical filesystem path that identifies this book.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Display title (derived: file stem for a file, directory name for a folder).
    pub fn title(&self) -> &str {
        &self.title
    }

    /// Leading page index of the last-viewed spread (0 when never opened).
    pub fn last_page(&self) -> usize {
        self.last_page
    }
}
```
- [x] **2.5 Run it (expect PASS):** `mise exec -- cargo nextest run -p gashuu-core book_derives_title`
  - Expected PASS: all three `book_*` tests pass. (`file_stem` on `My Folder Book` yields `My Folder Book`; on `/` yields `None` → lossy path string fallback.)
- [x] **2.6 Commit:** `git commit -m "feat(core): add Book type with derived title to library module"`

Progress: DONE 2026-06-02 - Book type and library module wiring added; focused nextest passed; title fallback regression covered.

---

## Task 3 — `library.rs`: `Library::new`/`books`/`add` (canonicalize + dedup + insertion order)

**Files:**
- Modify: `crates/gashuu-core/src/library.rs`
- Test: `crates/gashuu-core/src/library.rs` (inline tests)

- [ ] **3.1 Write failing test.** Add these tests inside the `#[cfg(test)] mod tests` block in `crates/gashuu-core/src/library.rs`:
```rust
    #[test]
    fn new_library_is_empty() {
        let lib = Library::new();
        assert!(lib.books().is_empty());
    }

    #[test]
    fn add_appends_in_insertion_order() {
        // Non-existent paths exercise the best-effort canonicalize fallback
        // (canonicalize fails → the path is used verbatim).
        let mut lib = Library::new();
        assert!(lib.add(PathBuf::from("/manga/a.cbz")));
        assert!(lib.add(PathBuf::from("/manga/b.cbz")));
        let titles: Vec<&str> = lib.books().iter().map(|b| b.title()).collect();
        assert_eq!(titles, vec!["a", "b"], "insertion order must be preserved");
    }

    #[test]
    fn add_dedups_by_path_and_returns_false() {
        let mut lib = Library::new();
        assert!(lib.add(PathBuf::from("/manga/a.cbz")));
        assert!(
            !lib.add(PathBuf::from("/manga/a.cbz")),
            "adding an existing path must be a no-op returning false"
        );
        assert_eq!(lib.books().len(), 1);
    }

    #[test]
    fn add_canonicalizes_existing_path_for_identity() {
        // A real file added via two different (but equivalent) path spellings
        // dedups, because `add` canonicalizes best-effort before comparing.
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("book.cbz");
        std::fs::write(&file, b"x").unwrap();
        // A path with a redundant `.` component canonicalizes to the same target.
        let dotted = dir.path().join(".").join("book.cbz");

        let mut lib = Library::new();
        assert!(lib.add(file.clone()));
        assert!(
            !lib.add(dotted),
            "an equivalent path must dedup after canonicalization"
        );
        assert_eq!(lib.books().len(), 1);
        // The stored path is the canonical form of the existing file.
        assert_eq!(lib.books()[0].path(), file.canonicalize().unwrap());
    }
```
- [ ] **3.2 Run it (expect FAIL):** `mise exec -- cargo nextest run -p gashuu-core new_library_is_empty add_appends_in_insertion_order add_dedups_by_path_and_returns_false add_canonicalizes_existing_path_for_identity`
  - Expected FAIL: `cannot find type 'Library'` / `no method named 'add'` — `Library` does not exist yet.
- [ ] **3.3 Minimal impl.** Insert the `Library` type below `impl Book` (above the test module) in `crates/gashuu-core/src/library.rs`:
```rust
/// An ordered, de-duplicated shelf of books. Insertion order is the carousel
/// order. Identity / dedup is on the canonical path (best-effort canonicalized at
/// `add` time). Mirrors `Settings::push_recent` discipline: one place owns the
/// invariants.
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Library {
    books: Vec<Book>,
}

impl Library {
    /// An empty library.
    pub fn new() -> Self {
        Self::default()
    }

    /// The books in insertion (carousel) order.
    pub fn books(&self) -> &[Book] {
        &self.books
    }

    /// Add `path` to the shelf. Canonicalizes best-effort (falling back to the
    /// path verbatim when canonicalization fails — e.g. a missing file), derives
    /// the title, and de-duplicates by canonical path. Returns `false` (no-op)
    /// when the canonical path is already present.
    pub fn add(&mut self, path: PathBuf) -> bool {
        let canonical = path.canonicalize().unwrap_or(path);
        if self.books.iter().any(|b| b.path() == canonical) {
            return false;
        }
        self.books.push(Book::from_path(canonical));
        true
    }
}
```
- [ ] **3.4 Run it (expect PASS):** `mise exec -- cargo nextest run -p gashuu-core new_library_is_empty add_appends_in_insertion_order add_dedups_by_path_and_returns_false add_canonicalizes_existing_path_for_identity`
  - Expected PASS: all four tests pass (canonicalize fallback for missing paths; dedup after canonicalization for the real file).
- [ ] **3.5 Commit:** `git commit -m "feat(core): add Library::new/books/add with canonicalize and dedup"`

---

## Task 4 — `library.rs`: `remove` + `last_page` + `set_last_page` + `is_available`

**Files:**
- Modify: `crates/gashuu-core/src/library.rs`
- Test: `crates/gashuu-core/src/library.rs` (inline tests)

- [ ] **4.1 Write failing test.** Add these tests inside the `#[cfg(test)] mod tests` block in `crates/gashuu-core/src/library.rs`:
```rust
    #[test]
    fn remove_existing_returns_true_and_drops_book() {
        let mut lib = Library::new();
        lib.add(PathBuf::from("/manga/a.cbz"));
        lib.add(PathBuf::from("/manga/b.cbz"));
        assert!(lib.remove(Path::new("/manga/a.cbz")));
        let titles: Vec<&str> = lib.books().iter().map(|b| b.title()).collect();
        assert_eq!(titles, vec!["b"]);
    }

    #[test]
    fn remove_absent_returns_false() {
        let mut lib = Library::new();
        lib.add(PathBuf::from("/manga/a.cbz"));
        assert!(!lib.remove(Path::new("/manga/missing.cbz")));
        assert_eq!(lib.books().len(), 1);
    }

    #[test]
    fn last_page_is_zero_for_unknown_path() {
        let lib = Library::new();
        assert_eq!(lib.last_page(Path::new("/manga/unknown.cbz")), 0);
    }

    #[test]
    fn set_last_page_updates_and_round_trips() {
        let mut lib = Library::new();
        lib.add(PathBuf::from("/manga/a.cbz"));
        assert!(
            lib.set_last_page(Path::new("/manga/a.cbz"), 7),
            "changing the value must return true"
        );
        assert_eq!(lib.last_page(Path::new("/manga/a.cbz")), 7);
    }

    #[test]
    fn set_last_page_false_when_absent() {
        let mut lib = Library::new();
        assert!(
            !lib.set_last_page(Path::new("/manga/missing.cbz"), 3),
            "absent path must return false"
        );
    }

    #[test]
    fn set_last_page_false_when_unchanged() {
        let mut lib = Library::new();
        lib.add(PathBuf::from("/manga/a.cbz"));
        assert!(lib.set_last_page(Path::new("/manga/a.cbz"), 4));
        assert!(
            !lib.set_last_page(Path::new("/manga/a.cbz"), 4),
            "an unchanged value must return false (mirrors jump_to 'did it change')"
        );
    }

    #[test]
    fn is_available_reflects_path_existence() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("real.cbz");
        std::fs::write(&file, b"x").unwrap();

        let mut lib = Library::new();
        lib.add(file.clone());
        lib.add(PathBuf::from("/manga/definitely-missing.cbz"));

        let real = &lib.books()[0];
        let missing = &lib.books()[1];
        assert!(Library::is_available(real), "existing path must be available");
        assert!(
            !Library::is_available(missing),
            "missing path must be unavailable (but still kept in the shelf)"
        );
    }
```
- [ ] **4.2 Run it (expect FAIL):** `mise exec -- cargo nextest run -p gashuu-core remove_existing_returns_true_and_drops_book set_last_page_false_when_unchanged is_available_reflects_path_existence`
  - Expected FAIL: `no method named 'remove' / 'last_page' / 'set_last_page' / 'is_available'` — the methods do not exist yet.
- [ ] **4.3 Minimal impl.** Add these methods to the `impl Library` block in `crates/gashuu-core/src/library.rs` (after `add`, before the closing brace):
```rust
    /// Remove the book identified by `path`. Returns `false` when absent.
    pub fn remove(&mut self, path: &Path) -> bool {
        let before = self.books.len();
        self.books.retain(|b| b.path() != path);
        self.books.len() != before
    }

    /// The last-viewed leading page index for `path` (0 when unknown).
    pub fn last_page(&self, path: &Path) -> usize {
        self.books
            .iter()
            .find(|b| b.path() == path)
            .map(Book::last_page)
            .unwrap_or(0)
    }

    /// Record `page` as the last-viewed leading page index for `path`. Returns
    /// `false` when the path is absent OR the value is unchanged (mirrors the
    /// `jump_to` "did it actually move" convention, so callers can skip a save).
    pub fn set_last_page(&mut self, path: &Path, page: usize) -> bool {
        match self.books.iter_mut().find(|b| b.path() == path) {
            Some(book) if book.last_page != page => {
                book.last_page = page;
                true
            }
            _ => false,
        }
    }

    /// Derived availability: whether the book's path currently resolves on disk.
    /// This is NOT stored — an unavailable book is kept (its reading position is
    /// preserved); removal is an explicit user action.
    pub fn is_available(book: &Book) -> bool {
        book.path().exists()
    }
```
- [ ] **4.4 Run it (expect PASS):** `mise exec -- cargo nextest run -p gashuu-core remove_existing_returns_true_and_drops_book remove_absent_returns_false last_page_is_zero_for_unknown_path set_last_page_updates_and_round_trips set_last_page_false_when_absent set_last_page_false_when_unchanged is_available_reflects_path_existence`
  - Expected PASS: all seven tests pass.
- [ ] **4.5 Commit:** `git commit -m "feat(core): add Library remove/last_page/set_last_page/is_available"`

---

## Task 5 — `library_store.rs`: `to_json`/`from_json` round-trip + version + re-export

**Files:**
- Create: `crates/gashuu-core/src/library_store.rs`
- Modify: `crates/gashuu-core/src/lib.rs`
- Test: `crates/gashuu-core/src/library_store.rs` (inline tests)

- [ ] **5.1 Write failing test.** Create `crates/gashuu-core/src/library_store.rs` with the header + the test module only:
```rust
//! Persistence for [`Library`], serialized to `library.json` in the OS data dir.
//!
//! Mirrors `settings.rs` discipline: path-taking primitives (`load_from`/`save_to`,
//! `tempfile`-testable) + OS convenience wrappers (`load`/`save`); a non-object-root
//! guard before any migrate; `version` parsed via `u32::try_from`. This crate stays
//! logging-free — corrupt-file recovery (warn + empty library) lives in the UI.

use crate::error::CoreError;
use crate::library::Library;
use directories::ProjectDirs;
use std::path::{Path, PathBuf};

/// On-disk schema version for `library.json`. Bump when the shape changes and add
/// a `migrate` step.
pub const LIBRARY_VERSION: u32 = 1;

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn to_json_then_from_json_round_trips() {
        let mut lib = Library::new();
        lib.add(PathBuf::from("/manga/a.cbz"));
        lib.add(PathBuf::from("/manga/b.cbz"));
        lib.set_last_page(Path::new("/manga/a.cbz"), 5);

        let json = lib.to_json();
        let parsed = Library::from_json(&json).unwrap();

        let original: Vec<(_, _, _)> = lib
            .books()
            .iter()
            .map(|b| (b.path().to_path_buf(), b.title().to_string(), b.last_page()))
            .collect();
        let restored: Vec<(_, _, _)> = parsed
            .books()
            .iter()
            .map(|b| (b.path().to_path_buf(), b.title().to_string(), b.last_page()))
            .collect();
        assert_eq!(original, restored);
    }

    #[test]
    fn to_json_emits_version_and_books() {
        let mut lib = Library::new();
        lib.add(PathBuf::from("/manga/a.cbz"));
        let value: serde_json::Value = serde_json::from_str(&lib.to_json()).unwrap();
        assert_eq!(value["version"].as_u64().unwrap() as u32, LIBRARY_VERSION);
        assert_eq!(value["books"][0]["title"].as_str().unwrap(), "a");
        assert_eq!(value["books"][0]["last_page"].as_u64().unwrap(), 0);
    }

    #[test]
    fn from_json_empty_object_yields_empty_library() {
        let parsed = Library::from_json("{}").unwrap();
        assert!(parsed.books().is_empty());
    }
}
```
- [ ] **5.2 Run it (expect FAIL):** `mise exec -- cargo nextest run -p gashuu-core to_json_then_from_json_round_trips`
  - Expected FAIL: `module 'library_store' not found` and `no method named 'to_json' / 'from_json'` — module is undeclared and the methods don't exist.
- [ ] **5.3 Minimal impl (part A: declare the module).** In `crates/gashuu-core/src/lib.rs`, add `pub mod library_store;` immediately AFTER the new `pub mod library;` line:
```rust
pub mod library_store;
```
- [ ] **5.4 Minimal impl (part B: write the JSON impl).** Insert this `impl Library` block + `migrate` fn ABOVE the `#[cfg(test)] mod tests` block in `crates/gashuu-core/src/library_store.rs`:
```rust
impl Library {
    /// Serialize to pretty JSON: `{ "version", "books": [...] }`.
    pub fn to_json(&self) -> String {
        // The wire shape wraps the book list with a schema version. `Library`'s
        // own `Serialize` covers `books`; the version is added explicitly here so
        // the stored document carries it for migration.
        let books = serde_json::to_value(self.books()).unwrap_or(serde_json::Value::Null);
        let value = serde_json::json!({
            "version": LIBRARY_VERSION,
            "books": books,
        });
        serde_json::to_string_pretty(&value).unwrap_or_else(|_| "{}".to_string())
    }

    /// Parse `library.json`, migrating older schema versions to the current shape.
    /// Rejects a non-object root BEFORE any migrate (a non-object would panic the
    /// map indexing in `migrate`); parses `version` via `u32::try_from` so a
    /// crafted huge value is treated as unknown rather than wrapping.
    pub fn from_json(json: &str) -> Result<Library, CoreError> {
        let value: serde_json::Value = serde_json::from_str(json).map_err(CoreError::Library)?;
        if !value.is_object() {
            // Reject non-object roots (`5`/`[]`/`"x"`/`true`/`null`) BEFORE migrate,
            // which indexes the value as a map and would panic on a non-object — the
            // same gotcha as `Settings::from_json`. Deserializing into a Map forces a
            // typed invalid-type error (guaranteed for a non-object), hence the
            // construction below.
            let err = serde_json::from_value::<serde_json::Map<String, serde_json::Value>>(value)
                .map_err(CoreError::Library)
                .expect_err("a non-object value cannot deserialize into a JSON object map");
            return Err(err);
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
        // The `books` array deserializes into `Library` (its only field). A missing
        // `books` key yields an empty library via `#[serde(default)]` on the Vec.
        serde_json::from_value(value).map_err(CoreError::Library)
    }
}

/// Upgrade a raw library JSON value from `from` to the current schema version.
/// With only v1 today this is the hook future schema changes plug into; it stamps
/// the version with the value reached by the migration chain.
fn migrate(mut value: serde_json::Value, from: u32) -> serde_json::Value {
    let version = from.max(1);
    value["version"] = serde_json::json!(version);
    value
}
```
  - **NOTE on `Library`'s `books` field deserialization:** `from_value` deserializes the whole object into `Library`. `Library`'s `books: Vec<Book>` field is NOT `#[serde(default)]` in `library.rs` as written in Task 3. To make a missing `books` key (and the `{}` test) work, add `#[serde(default)]` to the field. Apply this one-line change in `crates/gashuu-core/src/library.rs`:
```rust
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Library {
    #[serde(default)]
    books: Vec<Book>,
}
```
  - The extra `version` key in the JSON is ignored by serde when deserializing into `Library` (serde ignores unknown fields by default; do NOT add `#[serde(deny_unknown_fields)]`).
- [ ] **5.5 Add re-exports to `lib.rs`.** In `crates/gashuu-core/src/lib.rs`, add public re-exports. Place after the existing `pub use image_ops::{...};` line group (the `library` re-export goes alphabetically near it):
```rust
pub use library::{Book, Library};
pub use library_store::LIBRARY_VERSION;
```
- [ ] **5.6 Run it (expect PASS):** `mise exec -- cargo nextest run -p gashuu-core to_json_then_from_json_round_trips to_json_emits_version_and_books from_json_empty_object_yields_empty_library`
  - Expected PASS: all three tests pass; round-trip preserves path/title/last_page; `{}` yields an empty library via `#[serde(default)]`.
- [ ] **5.7 Commit:** `git commit -m "feat(core): add Library to_json/from_json with version and migrate hook"`

---

## Task 6 — `library_store.rs`: non-object-root guard + `u32::try_from` version

**Files:**
- Modify: `crates/gashuu-core/src/library_store.rs`
- Test: `crates/gashuu-core/src/library_store.rs` (inline tests)

- [ ] **6.1 Write failing test.** Add these tests inside the `#[cfg(test)] mod tests` block in `crates/gashuu-core/src/library_store.rs`:
```rust
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
        let result = Library::from_json("not json at all");
        assert!(matches!(result, Err(CoreError::Library(_))));
    }

    #[test]
    fn from_json_huge_version_is_treated_as_unknown_and_migrates() {
        // u32::MAX + 1 must NOT wrap to a small version via a truncating cast;
        // u32::try_from fails → version unknown (0) → migrate stamps the current
        // version. The document still loads (empty books).
        let json = serde_json::json!({
            "version": (u32::MAX as u64) + 1,
            "books": [],
        })
        .to_string();
        let parsed = Library::from_json(&json).unwrap();
        assert!(parsed.books().is_empty());
    }

    #[test]
    fn from_json_migrates_v0_to_current_version() {
        // A v0 document (predates the version stamp) loads and is migrated.
        let json = serde_json::json!({
            "version": 0,
            "books": [{ "path": "/manga/a.cbz", "title": "a", "last_page": 2 }],
        })
        .to_string();
        let parsed = Library::from_json(&json).unwrap();
        assert_eq!(parsed.books().len(), 1);
        assert_eq!(parsed.books()[0].last_page(), 2);
    }
```
- [ ] **6.2 Run it (expect FAIL):** `mise exec -- cargo nextest run -p gashuu-core from_json_non_object_root_errors from_json_huge_version_is_treated_as_unknown_and_migrates`
  - Expected FAIL: these tests FAIL only if the guard / `u32::try_from` logic from Task 5 were wrong. Since Task 5 already implemented `from_json` with the guard and `u32::try_from`, run this step to CONFIRM the behavior is pinned. If Task 5 is correct, expect these to PASS immediately — in which case this is a regression-pinning step (no impl change). If any FAIL, fix `from_json` per Task 5.4 before proceeding.
  - **(TDD note:** these tests pin behavior already implemented in Task 5; they exist to lock the contract's load-bearing safety rules — non-object guard + non-truncating version parse — against future edits. No new production code is expected.)
- [ ] **6.3 Minimal impl.** No production change expected (Task 5's `from_json` already satisfies these). If `from_json_non_object_root_errors` fails, verify the `!value.is_object()` guard runs BEFORE `migrate`. If `from_json_huge_version_*` fails, verify the version parse uses `u32::try_from(n).ok()`, not `as u32`.
- [ ] **6.4 Run it (expect PASS):** `mise exec -- cargo nextest run -p gashuu-core from_json_non_object_root_errors from_json_corrupt_text_errors from_json_huge_version_is_treated_as_unknown_and_migrates from_json_migrates_v0_to_current_version`
  - Expected PASS: all four tests pass.
- [ ] **6.5 Commit:** `git commit -m "test(core): pin Library non-object-root guard and version parse safety"`

---

## Task 7 — `library_store.rs`: `load_from`/`save_to` + OS `load`/`save` + `data_path`

**Files:**
- Modify: `crates/gashuu-core/src/library_store.rs`
- Test: `crates/gashuu-core/src/library_store.rs` (inline tests)

- [ ] **7.1 Write failing test.** Add these tests inside the `#[cfg(test)] mod tests` block in `crates/gashuu-core/src/library_store.rs`:
```rust
    #[test]
    fn load_from_missing_file_returns_empty_library() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("does-not-exist.json");
        let loaded = Library::load_from(&path).unwrap();
        assert!(
            loaded.books().is_empty(),
            "a missing library file must load as an empty library, not error"
        );
    }

    #[test]
    fn save_to_then_load_from_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        // Nested path to verify parent auto-creation (mirrors Settings::save_to).
        let path = dir.path().join("nested").join("sub").join("library.json");

        let mut original = Library::new();
        original.add(PathBuf::from("/manga/a.cbz"));
        original.add(PathBuf::from("/manga/b.cbz"));
        original.set_last_page(Path::new("/manga/b.cbz"), 9);
        original.save_to(&path).unwrap();

        let loaded = Library::load_from(&path).unwrap();
        let restored: Vec<(_, _, _)> = loaded
            .books()
            .iter()
            .map(|b| (b.path().to_path_buf(), b.title().to_string(), b.last_page()))
            .collect();
        assert_eq!(
            restored,
            vec![
                (PathBuf::from("/manga/a.cbz"), "a".to_string(), 0),
                (PathBuf::from("/manga/b.cbz"), "b".to_string(), 9),
            ]
        );
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
```
- [ ] **7.2 Run it (expect FAIL):** `mise exec -- cargo nextest run -p gashuu-core load_from_missing_file_returns_empty_library save_to_then_load_from_round_trips data_path_targets_gashuu_library_json`
  - Expected FAIL: `no function or associated item named 'load_from' / 'save_to' / 'data_path'` — they do not exist yet.
- [ ] **7.3 Minimal impl.** Add these methods to the `impl Library` block in `crates/gashuu-core/src/library_store.rs` (after `from_json`, before the closing brace):
```rust
    /// Resolve `library.json` in the OS data dir (creates nothing). Errors with
    /// `CoreError::NoDataDir` when the OS provides no data directory.
    pub fn data_path() -> Result<PathBuf, CoreError> {
        let dirs = ProjectDirs::from("", "", "gashuu").ok_or(CoreError::NoDataDir)?;
        Ok(dirs.data_dir().join("library.json"))
    }

    /// Load from the OS data path. Missing file => empty library (first run).
    pub fn load() -> Result<Library, CoreError> {
        Self::load_from(&Self::data_path()?)
    }

    /// Load from an explicit path. Missing => empty library; any other I/O error or
    /// malformed JSON => `Err`.
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

    /// Save to an explicit path, creating parent directories as needed.
    pub fn save_to(&self, path: &Path) -> Result<(), CoreError> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, self.to_json())?;
        Ok(())
    }
```
  - **NOTE:** `CoreError::from(e)` for the I/O error reuses the existing `Io(#[from] std::io::Error)` variant — a missing-file path is handled by the `NotFound` arm, so non-NotFound I/O surfaces as `CoreError::Io`, exactly like `Settings::load_from`.
- [ ] **7.4 Run it (expect PASS):** `mise exec -- cargo nextest run -p gashuu-core load_from_missing_file_returns_empty_library save_to_then_load_from_round_trips load_from_corrupt_file_errors data_path_targets_gashuu_library_json`
  - Expected PASS: all four tests pass; missing file → empty, round-trip preserves all fields, corrupt → `CoreError::Library`, data path ends in `library.json` under `gashuu`.
- [ ] **7.5 Commit:** `git commit -m "feat(core): add Library load/save (OS path + explicit path) for library.json"`

---

## Task 8 — `thumbnail_cache.rs`: pure `cache_key` + `lib.rs` wiring

**Files:**
- Create: `crates/gashuu-core/src/thumbnail_cache.rs`
- Modify: `crates/gashuu-core/src/lib.rs`
- Test: `crates/gashuu-core/src/thumbnail_cache.rs` (inline tests)

- [ ] **8.1 Write failing test.** Create `crates/gashuu-core/src/thumbnail_cache.rs` with the header + the test module first:
```rust
//! Disk cache for book cover thumbnails, keyed by `(path, mtime, max_side)`.
//!
//! Headless (no slint, no tracing). PR-0a ships the SKELETON: the struct, its
//! constructors, and the pure `cache_key`. `get` returns `None` and `put` is a
//! no-op so the public API COMPILES and downstream PRs (carousel, cover gen) can
//! link against it; PR-T fills the `get`/`put` bodies (read/decode and
//! encode/write over the cache directory).

use crate::error::CoreError;
use crate::image_ops::DecodedImage;
use directories::ProjectDirs;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cache_key_is_stable_for_same_inputs() {
        let p = Path::new("/manga/a.cbz");
        let k1 = cache_key(p, 1_700_000_000, 160);
        let k2 = cache_key(p, 1_700_000_000, 160);
        assert_eq!(k1, k2, "identical inputs must yield identical keys");
        assert!(!k1.is_empty());
        // The key is a hex stem: lowercase hex digits only (safe as a filename).
        assert!(
            k1.bytes().all(|b| b.is_ascii_hexdigit()),
            "key must be hex: {k1}"
        );
    }

    #[test]
    fn cache_key_differs_on_mtime() {
        let p = Path::new("/manga/a.cbz");
        assert_ne!(
            cache_key(p, 1, 160),
            cache_key(p, 2, 160),
            "a changed mtime must change the key so a stale cover regenerates"
        );
    }

    #[test]
    fn cache_key_differs_on_max_side() {
        let p = Path::new("/manga/a.cbz");
        assert_ne!(
            cache_key(p, 1, 160),
            cache_key(p, 1, 320),
            "a different thumbnail size must change the key"
        );
    }

    #[test]
    fn cache_key_differs_on_path() {
        assert_ne!(
            cache_key(Path::new("/manga/a.cbz"), 1, 160),
            cache_key(Path::new("/manga/b.cbz"), 1, 160),
            "different books must have different keys"
        );
    }
}
```
- [ ] **8.2 Run it (expect FAIL):** `mise exec -- cargo nextest run -p gashuu-core cache_key_is_stable_for_same_inputs`
  - Expected FAIL: `module 'thumbnail_cache' not found` / `cannot find function 'cache_key'` — undeclared module and missing fn.
- [ ] **8.3 Minimal impl (part A: declare the module).** In `crates/gashuu-core/src/lib.rs`, add `pub mod thumbnail_cache;`. Place it after the existing `pub mod thumbnail;` line (alphabetical: `thumbnail` then `thumbnail_cache`):
```rust
pub mod thumbnail_cache;
```
- [ ] **8.4 Minimal impl (part B: write `cache_key`).** Insert this ABOVE the `#[cfg(test)] mod tests` block in `crates/gashuu-core/src/thumbnail_cache.rs`:
```rust
/// Pure key derivation for a cover thumbnail: a stable lowercase-hex filename
/// stem from `(path, mtime_secs, max_side)`.
///
/// The caller resolves `mtime_secs` (this fn does NO I/O), so the same source +
/// size yields the same key; a changed file (new mtime) or a different requested
/// size yields a different key, which makes the on-disk cover regenerate
/// automatically. Hashing uses the std `DefaultHasher` (SipHash) — no extra
/// dependency. The key is used only as a cache filename: it does not need to be
/// cryptographic, and a hash-impl change across std versions merely invalidates
/// the mtime-keyed cache (which lives in the OS cache dir), never user data.
pub fn cache_key(path: &Path, mtime_secs: i64, max_side: u32) -> String {
    let mut hasher = DefaultHasher::new();
    path.hash(&mut hasher);
    mtime_secs.hash(&mut hasher);
    max_side.hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}
```
- [ ] **8.5 Run it (expect PASS):** `mise exec -- cargo nextest run -p gashuu-core cache_key_is_stable_for_same_inputs cache_key_differs_on_mtime cache_key_differs_on_max_side cache_key_differs_on_path`
  - Expected PASS: all four tests pass (stable + hex; differs on mtime/max_side/path).
- [ ] **8.6 Commit:** `git commit -m "feat(core): add pure thumbnail cache_key derivation"`

---

## Task 9 — `thumbnail_cache.rs`: `ThumbnailCache` struct + `new`/`with_dir` + skeleton `get`/`put`

**Files:**
- Modify: `crates/gashuu-core/src/thumbnail_cache.rs`
- Modify: `crates/gashuu-core/src/lib.rs`
- Test: `crates/gashuu-core/src/thumbnail_cache.rs` (inline tests)

- [ ] **9.1 Write failing test.** Add these tests inside the `#[cfg(test)] mod tests` block in `crates/gashuu-core/src/thumbnail_cache.rs`:
```rust
    #[test]
    fn with_dir_constructs_and_get_returns_none_skeleton() {
        // PR-0a skeleton contract: `get` always returns None (PR-T fills the body).
        let dir = tempfile::tempdir().unwrap();
        let cache = ThumbnailCache::with_dir(dir.path().to_path_buf());
        assert!(
            cache.get("deadbeef").is_none(),
            "skeleton get must return None until PR-T implements it"
        );
    }

    #[test]
    fn put_is_noop_skeleton_and_writes_nothing() {
        // PR-0a skeleton contract: `put` succeeds without touching the directory.
        let dir = tempfile::tempdir().unwrap();
        let cache = ThumbnailCache::with_dir(dir.path().to_path_buf());
        let img = DecodedImage::new(vec![0u8; 4], 1, 1).unwrap();
        assert!(
            cache.put("deadbeef", &img).is_ok(),
            "skeleton put must return Ok"
        );
        // No file was written (skeleton no-op): the directory stays empty.
        let entries: Vec<_> = std::fs::read_dir(dir.path()).unwrap().collect();
        assert!(
            entries.is_empty(),
            "skeleton put must not write any file yet"
        );
    }
```
- [ ] **9.2 Run it (expect FAIL):** `mise exec -- cargo nextest run -p gashuu-core with_dir_constructs_and_get_returns_none_skeleton put_is_noop_skeleton_and_writes_nothing`
  - Expected FAIL: `cannot find type 'ThumbnailCache'` / `no method named 'with_dir' / 'get' / 'put'` — the struct does not exist yet.
- [ ] **9.3 Minimal impl.** Insert this `ThumbnailCache` struct + impl ABOVE the `cache_key` fn (or directly below it) in `crates/gashuu-core/src/thumbnail_cache.rs`:
```rust
/// A disk cache for book cover thumbnails living under the OS cache directory.
///
/// PR-0a SKELETON: holds the cache directory and exposes the frozen API so the
/// carousel / cover-generation PRs link against it. `get` returns `None` and
/// `put` is a no-op; PR-T implements them (read `<dir>/<key>.png` → decode;
/// PNG-encode → write `<dir>/<key>.png`).
pub struct ThumbnailCache {
    #[allow(dead_code)] // Read by PR-T's get/put bodies; unused in the PR-0a skeleton.
    dir: PathBuf,
}

impl ThumbnailCache {
    /// Construct over the OS cache dir (`<cache>/gashuu/covers`). Errors with
    /// `CoreError::NoDataDir` when the OS provides no cache directory.
    pub fn new() -> Result<ThumbnailCache, CoreError> {
        let dirs = ProjectDirs::from("", "", "gashuu").ok_or(CoreError::NoDataDir)?;
        Ok(Self {
            dir: dirs.cache_dir().join("covers"),
        })
    }

    /// Construct over an explicit directory — a tempfile-testable seam.
    pub fn with_dir(dir: PathBuf) -> ThumbnailCache {
        Self { dir }
    }

    /// Look up a cached cover by key. PR-0a SKELETON: always `None` (PR-T reads
    /// `<dir>/<key>.png` and decodes it).
    pub fn get(&self, _key: &str) -> Option<DecodedImage> {
        None
    }

    /// Store a cover under `key`. PR-0a SKELETON: no-op returning `Ok` (PR-T
    /// PNG-encodes `img` and writes `<dir>/<key>.png`).
    pub fn put(&self, _key: &str, _img: &DecodedImage) -> Result<(), CoreError> {
        Ok(())
    }
}
```
  - **NOTE on `#[allow(dead_code)]`:** the `dir` field is set by both constructors but unread by the skeleton `get`/`put`; `clippy -D warnings` (and even `cargo build` for an unused field) would otherwise flag it. The `#[allow(dead_code)]` is intentional and documented in place, matching the project's "test-only / future-use accessor" convention in `docs/patterns.md`. PR-T removes the allow when it reads `dir`.
  - **NOTE on `new()` error variant:** the contract's skeleton signature returns `CoreError`. There is no dedicated "no cache dir" variant; reuse `CoreError::NoDataDir` (the closest existing "OS dir unavailable" variant added in Task 1) rather than introducing a third dir-error variant. `new()` is not exercised by a unit test (it depends on the live OS cache dir); it is covered for compilation only, with `with_dir` carrying the testable behavior — matching how `Settings::config_path` is the only OS-path test and the rest use `_from` seams.
- [ ] **9.4 Add re-exports to `lib.rs`.** In `crates/gashuu-core/src/lib.rs`, add the thumbnail-cache re-export (place after the existing `pub use thumbnail::{...};` line):
```rust
pub use thumbnail_cache::{cache_key, ThumbnailCache};
```
- [ ] **9.5 Run it (expect PASS):** `mise exec -- cargo nextest run -p gashuu-core with_dir_constructs_and_get_returns_none_skeleton put_is_noop_skeleton_and_writes_nothing`
  - Expected PASS: both tests pass; `with_dir` constructs, `get` → `None`, `put` → `Ok` and writes nothing.
- [ ] **9.6 Commit:** `git commit -m "feat(core): add ThumbnailCache skeleton (struct + new/with_dir + no-op get/put)"`

---

## Task 10 — Final gates (all three must be green)

**Files:**
- (no source changes unless a gate fails; fix forward and re-run)

- [ ] **10.1 Format gate:** `mise exec -- cargo fmt --check`
  - Expected: no output, exit 0. If it reports diffs, run `mise exec -- cargo fmt`, review, and re-run until clean.
- [ ] **10.2 Clippy gate:** `mise exec -- cargo clippy --workspace --all-targets -- -D warnings`
  - Expected: no warnings, exit 0. Likely-to-surface lints and their fixes:
    - unused `dir` field → already covered by the documented `#[allow(dead_code)]` in Task 9.
    - `clippy::new_without_default` on `ThumbnailCache::new` (it returns `Result`, so this lint does NOT fire — `new` is fallible; no `Default` is expected). If it unexpectedly fires, add a one-line `#[allow(clippy::new_without_default)]` with a comment that `new` is fallible.
- [ ] **10.3 Test gate:** `mise exec -- cargo nextest run --workspace --profile ci`
  - Expected: all tests pass across the workspace (the new `library`, `library_store`, `thumbnail_cache`, and `error` tests plus all pre-existing tests). The `gashuu` UI crate is unaffected (PR-0a is core-only) and must still pass.
- [ ] **10.4 Commit (only if any gate required a fix):** `git commit -m "chore(core): satisfy fmt/clippy/test gates for PR-0a"`
  - If all three gates were already green after Task 9 with no edits, skip this commit.

---

## Done-when checklist

- [ ] `crates/gashuu-core/src/library.rs` exists with `Book` (private fields, derived title, `path`/`title`/`last_page`) and `Library` (`new`/`books`/`add`/`remove`/`last_page`/`set_last_page`/`is_available`), matching the frozen signatures exactly.
- [ ] `crates/gashuu-core/src/library_store.rs` exists with `LIBRARY_VERSION`, `to_json`/`from_json` (non-object guard before migrate + `u32::try_from` version), `load_from`/`save_to`, `load`/`save`, `data_path`.
- [ ] `crates/gashuu-core/src/thumbnail_cache.rs` exists with pure `cache_key`, `ThumbnailCache` struct, `new`/`with_dir`, and skeleton `get`(→`None`)/`put`(→`Ok`, no-op).
- [ ] `crates/gashuu-core/src/error.rs` has `CoreError::Library(serde_json::Error)` (NO `#[from]`) and `CoreError::NoDataDir`.
- [ ] `crates/gashuu-core/src/lib.rs` declares `library`, `library_store`, `thumbnail_cache` and re-exports `Book`, `Library`, `LIBRARY_VERSION`, `cache_key`, `ThumbnailCache`.
- [ ] All three gates green; every commit is a one-line conventional message with no `Co-Authored-By` and no body.
