//! A naturally ordered, de-duplicated shelf keyed by opaque book IDs.
//!
//! Callers supply identity and display titles. On desktop, the load path
//! explicitly re-resolves legacy locations before normalizing the shelf.

use crate::reading_progress::ReadingProgress;
use crate::view_override::ViewOverride;
use crate::BookId;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
#[cfg(not(target_arch = "wasm32"))]
use std::collections::HashMap;
use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};

/// One book in the shelf, with a caller-supplied title and stored reading state.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(from = "BookRecord", into = "BookRecord")]
pub struct Book {
    id: BookId,
    title: String,
    progress: ReadingProgress,
    overrides: ViewOverride,
}

/// Version 1 book fields in their on-disk order.
#[derive(Serialize, Deserialize)]
struct BookRecord {
    path: BookId,
    title: String,
    #[serde(default)]
    last_page: usize,
    #[serde(default, with = "page_count_serde")]
    page_count: Option<NonZeroUsize>,
    #[serde(default, skip_serializing_if = "ViewOverride::is_empty")]
    overrides: ViewOverride,
}

impl From<BookRecord> for Book {
    fn from(record: BookRecord) -> Self {
        Self {
            id: record.path,
            title: record.title,
            progress: ReadingProgress::new(record.last_page, record.page_count),
            overrides: record.overrides,
        }
    }
}

impl From<Book> for BookRecord {
    fn from(book: Book) -> Self {
        Self {
            path: book.id,
            title: book.title,
            last_page: book.progress.last_viewed(),
            page_count: book.progress.total_count(),
            overrides: book.overrides,
        }
    }
}

/// Serde shim for [`BookRecord::page_count`]: the in-memory `Option<NonZeroUsize>` is
/// persisted as a bare `usize` (the historical on-disk shape), with `0` standing
/// for the unknown state (`None`) and any positive `n` for `Some(n)`. This keeps
/// `library.json` byte-compatible while the domain type forbids the out-of-band
/// `0`. `None` is always emitted (as `0`), never skipped, so the object keeps its
/// fixed key set.
mod page_count_serde {
    use serde::{Deserialize, Deserializer, Serialize, Serializer};
    use std::num::NonZeroUsize;

    pub(super) fn serialize<S: Serializer>(
        value: &Option<NonZeroUsize>,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        value
            .map_or(0usize, NonZeroUsize::get)
            .serialize(serializer)
    }

    pub(super) fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Option<NonZeroUsize>, D::Error> {
        Ok(NonZeroUsize::new(usize::deserialize(deserializer)?))
    }
}

/// Derive a book title from the directory name or file stem, using the
/// full path as a fallback when the selected component is missing or empty.
pub fn display_title(path: &Path, is_dir: bool) -> String {
    if is_dir {
        path.file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .filter(|s| !s.is_empty())
    } else {
        path.file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .filter(|s| !s.is_empty())
    }
    .unwrap_or_else(|| path.to_string_lossy().into_owned())
}

/// Resolve a desktop location for use as an ID. Keep the original spelling
/// when canonicalization fails or yields a path that cannot serve as a UTF-8 ID.
pub fn canonical_identity(path: &Path) -> PathBuf {
    path.canonicalize()
        .ok()
        .filter(|canonical| canonical.to_str().is_some())
        .unwrap_or_else(|| path.to_path_buf())
}

impl Book {
    pub(crate) fn new(id: BookId, title: String) -> Self {
        Self {
            id,
            title,
            progress: ReadingProgress::default(),
            overrides: ViewOverride::none(),
        }
    }

    pub fn id(&self) -> &BookId {
        &self.id
    }

    /// The caller-supplied display title.
    pub fn title(&self) -> &str {
        &self.title
    }

    /// Resume page index (0 when never opened).
    pub fn resume_page(&self) -> usize {
        self.progress.last_viewed()
    }

    /// Total page count cached at open time, or `None` when unknown.
    pub fn page_count_opt(&self) -> Option<usize> {
        self.progress.total()
    }

    /// This book's stored reading progress.
    pub fn progress(&self) -> ReadingProgress {
        self.progress
    }

    /// This book's view preference overrides (all-None when it inherits global).
    pub fn overrides(&self) -> ViewOverride {
        self.overrides
    }
}

fn book_order(a: &Book, b: &Book) -> std::cmp::Ordering {
    crate::ordering::natural_cmp(a.title(), b.title())
        .then_with(|| a.id().as_str().cmp(b.id().as_str()))
}

/// Collapse one resolution group (books resolving to the same `key`) into a single
/// survivor. The first member is the base — it keeps its `title` (the earliest-added
/// identity) — and the rest fold in under the deterministic merge policy: `resume_page`
/// takes the max, `page_count` the first `Some` in vec order, `overrides` the first
/// non-empty in vec order. The survivor's progress is constructed from both values
/// because the max resume and first-`Some` count can come from different members.
/// The survivor's ID is finally set to `key`, which upgrades a now-resolvable
/// raw path even for a group of one. `members` is non-empty by
/// construction (a group is only created when a member is pushed).
#[cfg(not(target_arch = "wasm32"))]
fn merge_group(key: BookId, members: Vec<Book>) -> Book {
    let mut iter = members.into_iter();
    let mut survivor = iter
        .next()
        .expect("a resolution group has at least one member");
    let mut last_viewed = survivor.progress.last_viewed();
    let mut total = survivor.progress.total_count();
    for member in iter {
        last_viewed = last_viewed.max(member.progress.last_viewed());
        total = total.or(member.progress.total_count());
        if survivor.overrides.is_empty() {
            survivor.overrides = member.overrides;
        }
    }
    survivor.id = key;
    survivor.progress = ReadingProgress::new(last_viewed, total);
    survivor
}

/// An ordered shelf, de-duplicated by book ID and sorted by natural title
/// order with the raw ID as the tie-break.
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Library {
    #[serde(default)]
    books: Vec<Book>,
    /// The most recently opened book. When set, its ID is present in the shelf.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    last_opened: Option<BookId>,
}

/// Deduplicated removal inputs, in ID order within each partition.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct RemovalReport {
    pub removed: Vec<BookId>,
    pub not_found: Vec<BookId>,
}

/// Outcome of [`Library::register_opened`]: where to resume reading and whether the
/// stored reading progress changed (so the caller can decide whether to rebuild the
/// carousel). A pure value — no I/O implied.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct OpenRegistration {
    /// The position to resume at (the book's recorded `ReadingProgress`).
    pub resume: ReadingProgress,
    /// Whether the measured count changed the stored progress (total or resume index).
    pub count_changed: bool,
}

impl Library {
    /// Look up a book by ID.
    pub fn book(&self, id: &BookId) -> Option<&Book> {
        self.books.iter().find(|book| book.id() == id)
    }

    fn book_mut(&mut self, id: &BookId) -> Option<&mut Book> {
        self.books.iter_mut().find(|book| book.id() == id)
    }

    /// An empty library.
    pub fn new() -> Self {
        Self::default()
    }

    /// The books in natural title (carousel) order, with raw ID tie-break.
    pub fn books(&self) -> &[Book] {
        &self.books
    }

    /// The ID of the most recently opened book, if any.
    pub fn last_opened(&self) -> Option<&BookId> {
        self.last_opened.as_ref()
    }

    /// Clear all persisted reading-library state. Returns `true` when either
    /// shelved books or `last_opened` changed, so callers can skip a save for an
    /// already-empty library.
    pub fn clear(&mut self) -> bool {
        let changed = !self.books.is_empty() || self.last_opened.is_some();
        self.books.clear();
        self.last_opened = None;
        changed
    }

    /// The most recently opened book's ID if it is still on the shelf.
    pub fn bookmark(&self) -> Option<&BookId> {
        self.book(self.last_opened()?).map(Book::id)
    }

    /// Add a book and return its stored ID. A duplicate leaves the shelf unchanged.
    pub fn add(&mut self, id: BookId, title: String) -> Option<&BookId> {
        if self.book(&id).is_some() {
            return None;
        }
        self.books.push(Book::new(id.clone(), title));
        self.books.sort_by(book_order);
        self.book(&id).map(Book::id)
    }

    /// Add a batch, skipping duplicate IDs. Return new IDs in input order and
    /// sort the shelf once after insertion.
    pub fn add_many(&mut self, books: Vec<(BookId, String)>) -> Vec<BookId> {
        let mut added = Vec::new();
        for (id, title) in books {
            if self.book(&id).is_none() {
                self.books.push(Book::new(id.clone(), title));
                added.push(id);
            }
        }
        self.books.sort_by(book_order);
        added
    }

    /// Re-canonicalize stored book paths and merge entries that resolve to the
    /// same physical file into one. This self-heals a `library.json` written
    /// before book identity was the add-time canonical path: a book added while
    /// its file was missing keeps a raw/relative/`..`-bearing path, so when the
    /// same file is later added once it resolves, the two divergent spellings end
    /// up as two `Book` entries for one file with split reading progress.
    ///
    /// Unlike [`normalize`](Library::normalize) this performs filesystem I/O (a
    /// per-book `canonicalize`) — it is the aggregate's ONE such exception,
    /// because there is no identity a caller could supply for a stored spelling
    /// that must be re-resolved. It is therefore deliberately a SEPARATE
    /// pub(crate) routine invoked explicitly from `library_store::from_json`
    /// before `normalize`, not folded into the pure `normalize`.
    ///
    /// Resolution uses [`canonical_identity`]. A missing file or a non-UTF-8
    /// canonical form keeps the raw path as
    /// its key: it can never be merged away and its path is preserved verbatim
    /// (a singleton group whose key equals its own raw path). Books are grouped
    /// by key in first-seen (vec) order; each group collapses to one survivor
    /// whose ID is set to the canonical key (upgrading a now-resolvable raw
    /// path even for a group of one). Cross-member field merge is deterministic:
    /// progress combines the maximum resume with the first known count in vec order,
    /// `overrides` = first non-empty in vec order, `title` = the first member's
    /// title.
    /// `last_opened` is repointed from a merged-away / pre-upgrade spelling to the
    /// survivor's canonical key (a genuine orphan is left for `normalize` to clear).
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn recanonicalize_and_merge(&mut self) {
        let books = std::mem::take(&mut self.books);
        // First-seen key order, the group members, and a remap from each book's
        // ORIGINAL stored path to its group's canonical key (for `last_opened`).
        let mut order: Vec<BookId> = Vec::new();
        let mut groups: HashMap<BookId, Vec<Book>> = HashMap::new();
        let mut remap: HashMap<BookId, BookId> = HashMap::new();
        for book in books {
            let canonical = canonical_identity(Path::new(book.id().as_str()));
            let key = BookId::from_canonical_path(&canonical).unwrap_or_else(|_| book.id().clone());
            remap.insert(book.id().clone(), key.clone());
            if !groups.contains_key(&key) {
                order.push(key.clone());
            }
            groups.entry(key).or_default().push(book);
        }
        let mut survivors = Vec::with_capacity(order.len());
        for key in order {
            let members = groups.remove(&key).expect("group for first-seen key");
            survivors.push(merge_group(key, members));
        }
        self.books = survivors;
        // Repoint a `last_opened` spelled as a merged-away / pre-upgrade path to its
        // survivor's canonical key. A path absent from `remap` is left for normalize.
        if let Some(last_opened) = self.last_opened.take() {
            self.last_opened = Some(remap.get(&last_opened).cloned().unwrap_or(last_opened));
        }
        self.books.sort_by(book_order);
    }

    /// Re-sort the shelf into natural title order (with raw ID tie-break)
    /// and enforce the `last_opened` invariant. Called on load
    /// (`LibraryStore::load`) so libraries persisted before natural
    /// ordering converge to the canonical sort on the next save; it also repairs
    /// any otherwise-unsorted `books` vec and clears an orphan `last_opened`
    /// (an ID that is no longer present in `books`).
    pub(crate) fn normalize(&mut self) {
        self.books.sort_by(book_order);
        // Clear a `last_opened` whose ID is no longer in the shelf (e.g.
        // after an external edit removed a book without going through `remove`).
        if let Some(ref p) = self.last_opened {
            if self.book(p).is_none() {
                self.last_opened = None;
            }
        }
    }

    /// Remove a book by ID and clear its bookmark if it was the last opened.
    pub fn remove(&mut self, id: &BookId) -> bool {
        let before = self.books.len();
        self.books.retain(|book| book.id() != id);
        let removed = self.books.len() != before;
        if removed && self.last_opened.as_ref() == Some(id) {
            self.last_opened = None;
        }
        removed
    }

    /// Remove a set of IDs in one retain pass, preserving shelf order.
    pub fn remove_many(&mut self, ids: &[BookId]) -> RemovalReport {
        let requested: BTreeSet<&BookId> = ids.iter().collect();
        let mut report = RemovalReport::default();
        for &id in &requested {
            if self.book(id).is_some() {
                report.removed.push(id.clone());
            } else {
                report.not_found.push(id.clone());
            }
        }
        self.books.retain(|book| !requested.contains(book.id()));
        if self.last_opened().is_some_and(|id| requested.contains(id)) {
            self.last_opened = None;
        }
        report
    }

    /// Re-insert previously removed `Book` entries AND restore `last_opened`,
    /// returning to a truly byte-identical pre-removal state.
    ///
    /// Unlike [`add`](Library::add), which creates a fresh book
    /// (losing `resume_page` / `page_count` / `overrides`), this re-inserts WHOLE
    /// `Book` values, so it can undo a [`remove_many`](Library::remove_many) with
    /// no data loss. It is the rollback primitive: a caller that removed a set of
    /// books, kept clones AND the pre-removal `last_opened`, and then failed to
    /// persist can hand both back to recover the exact pre-removal shelf
    /// (byte-identical re-serialization).
    ///
    /// Skips IDs already on the shelf, then
    /// re-sorts via the same `book_order` invariant `add` uses — the aggregate
    /// owns its ordering, callers never re-sort. Empty input is a no-op.
    ///
    /// `last_opened` is restored only when the passed ID is present on the
    /// re-inserted shelf, preserving the invariant that `last_opened`, when
    /// `Some`, always points at a shelved book (a stale bookmark is dropped).
    pub fn restore(&mut self, books: Vec<Book>, last_opened: Option<BookId>) {
        for book in books {
            if self.book(book.id()).is_none() {
                self.books.push(book);
            }
        }
        self.books.sort_by(book_order);
        // Restore the bookmark, but only if it points at a book now on the shelf,
        // upholding the `last_opened`-is-a-member invariant.
        if let Some(p) = last_opened {
            if self.book(&p).is_some() {
                self.last_opened = Some(p);
            }
        }
    }

    /// The resume page index for `id` (0 when unknown).
    pub fn resume_page(&self, id: &BookId) -> usize {
        self.book(id).map(Book::resume_page).unwrap_or(0)
    }

    /// Record a resume index, clamped to the final page when the total is known.
    /// Returns true only when the book exists and its stored progress changes.
    pub fn set_resume_page(&mut self, id: &BookId, page: usize) -> bool {
        let Some(book) = self.book_mut(id) else {
            return false;
        };
        let next = book.progress.with_last_viewed(page);
        let changed = next != book.progress;
        book.progress = next;
        changed
    }

    /// Record a measured total, clamping the resume index to the new final page.
    /// Returns true only when the book exists and its stored progress changes.
    pub fn set_page_count(&mut self, id: &BookId, count: NonZeroUsize) -> bool {
        let Some(book) = self.book_mut(id) else {
            return false;
        };
        let next = book.progress.with_total(count);
        let changed = next != book.progress;
        book.progress = next;
        changed
    }

    /// Record the view overrides for `id`. Returns `false` when the ID is
    /// absent OR the value is unchanged (mirrors `set_resume_page`/`set_page_count`
    /// so callers can skip a save). The aggregate owns this mutation; `Book` has
    /// no setter.
    pub fn set_overrides(&mut self, id: &BookId, overrides: ViewOverride) -> bool {
        match self.book_mut(id) {
            Some(book) if book.overrides != overrides => {
                book.overrides = overrides;
                true
            }
            _ => false,
        }
    }

    /// The view overrides for `id`, or an all-None override when `id` is
    /// absent (so callers always get a value to `resolve`).
    pub fn overrides_for(&self, id: &BookId) -> ViewOverride {
        self.book(id).map(Book::overrides).unwrap_or_default()
    }

    /// Register a book using the supplied title on first open. Update its
    /// progress when the page count is known and return where to resume.
    pub fn register_opened(
        &mut self,
        id: &BookId,
        title: &str,
        page_count: Option<NonZeroUsize>,
    ) -> OpenRegistration {
        if self.book(id).is_none() {
            self.add(id.clone(), title.to_owned());
        }
        let count_changed = page_count.is_some_and(|count| self.set_page_count(id, count));
        let resume = self
            .book(id)
            .expect("registered book must be present")
            .progress();
        self.last_opened = Some(id.clone());
        OpenRegistration {
            resume,
            count_changed,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn book_derives_title_from_file_stem() {
        let book = book_from_path(Path::new("/manga/Cool Title.cbz"), false);
        assert_eq!(book.title(), "Cool Title");
        assert_eq!(book.id(), &BookId::new("/manga/Cool Title.cbz"));
        assert_eq!(book.resume_page(), 0);
    }

    #[test]
    fn book_derives_title_from_dir_name_when_no_extension() {
        // A folder book has no extension; the title is the directory name.
        let book = book_from_path(Path::new("/manga/My Folder Book"), true);
        assert_eq!(book.title(), "My Folder Book");
    }

    #[test]
    fn book_derives_title_from_dotted_existing_dir_name() {
        let dir = tempfile::tempdir().unwrap();
        let folder = dir.path().join("Series.1997");
        std::fs::create_dir(&folder).unwrap();
        let book = book_from_path(&folder, true);
        assert_eq!(book.title(), "Series.1997");
    }

    #[test]
    fn book_title_falls_back_to_path_string_when_no_stem() {
        // Pathological path with no file_stem (e.g. root) falls back to the lossy
        // path string so the title is never empty.
        let book = book_from_path(Path::new("/"), false);
        assert!(!book.title().is_empty());
    }

    // --- display_title tests (the pure title rule) ---

    #[test]
    fn display_title_uses_dir_name_for_real_folder() {
        // A real existing folder: the rule takes the directory name, not the stem
        // (which would strip a dotted suffix).
        let dir = tempfile::tempdir().unwrap();
        let folder = dir.path().join("Series.1997");
        std::fs::create_dir(&folder).unwrap();
        assert_eq!(display_title(&folder, true), "Series.1997");
    }

    #[test]
    fn display_title_uses_file_stem_for_archive_path() {
        // A file path (non-directory): the rule takes the file stem, dropping the
        // extension.
        assert_eq!(
            display_title(Path::new("/manga/Cool Title.cbz"), false),
            "Cool Title"
        );
    }

    #[test]
    fn display_title_keeps_leading_dot_name_as_stem() {
        // A dotfile like `.cbz` keeps the whole `.cbz` as its (non-empty) stem, so the
        // empty filter never fires and the title is the dotfile name, not the fallback.
        assert_eq!(display_title(Path::new("/manga/.cbz"), false), ".cbz");
    }

    #[test]
    fn display_title_falls_back_to_lossy_path_when_no_file_component() {
        // A non-dir path whose `file_stem` is None (a trailing `..`) hits the fallback
        // arm: the lossy full path string is used so the title is never empty.
        let title = display_title(Path::new("/manga/.."), false);
        assert!(!title.is_empty());
        assert_eq!(title, "/manga/..");
    }

    #[test]
    fn display_title_falls_back_to_lossy_path_for_root() {
        // A root-ish path has no file_name / file_stem at all, so the rule falls
        // back to the lossy full path string.
        let title = display_title(Path::new("/"), false);
        assert!(!title.is_empty());
        assert_eq!(title, "/");
    }

    fn book_from_path(path: &Path, is_dir: bool) -> Book {
        Book::new(
            BookId::from_canonical_path(path).unwrap(),
            display_title(path, is_dir),
        )
    }

    fn add(lib: &mut Library, raw: &str) -> BookId {
        lib.add(BookId::new(raw), display_title(Path::new(raw), false))
            .unwrap()
            .clone()
    }

    fn title(id: &BookId) -> String {
        display_title(Path::new(id.as_str()), false)
    }

    #[test]
    fn display_title_is_pure_given_is_dir() {
        let path = Path::new("/manga/Series.1997");
        assert_eq!(display_title(path, true), "Series.1997");
        assert_eq!(display_title(path, false), "Series");
    }

    #[test]
    fn book_lookup_by_id() {
        let mut lib = Library::new();
        let id = add(&mut lib, "opaque-id");
        assert_eq!(lib.book(&id).unwrap().id(), &id);
        assert!(lib.book(&BookId::new("missing")).is_none());
    }

    #[test]
    fn remove_many_report_order_matches_path_order() {
        let mut lib = Library::new();
        let spaced = add(&mut lib, "/m/A 2.cbz");
        let nested = add(&mut lib, "/m/A/1.cbz");
        let report = lib.remove_many(&[spaced.clone(), nested.clone()]);
        assert_eq!(report.removed, vec![nested, spaced]);
    }

    #[test]
    fn register_opened_uses_caller_title_for_a_new_book_and_keeps_stored_title_on_reopen() {
        let mut lib = Library::new();
        let id = BookId::new("opaque-id");
        lib.register_opened(&id, "First title", None);
        assert_eq!(lib.book(&id).unwrap().title(), "First title");
        lib.register_opened(&id, "Changed title", None);
        assert_eq!(lib.book(&id).unwrap().title(), "First title");
    }

    #[test]
    fn canonical_identity_returns_the_canonical_path_for_a_real_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("book.cbz");
        std::fs::write(&path, b"").unwrap();

        assert_eq!(canonical_identity(&path), path.canonicalize().unwrap());
    }

    #[test]
    fn canonical_identity_falls_back_for_a_missing_path() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("missing.cbz");

        assert_eq!(canonical_identity(&path), path);
    }

    #[cfg(unix)]
    fn utf8_symlink_to_non_utf8_target() -> (tempfile::TempDir, PathBuf) {
        // Unix permits arbitrary path bytes and symlinks, which are needed to
        // reproduce a UTF-8 user selection resolving to a non-UTF-8 identity.
        use std::ffi::OsStr;
        use std::os::unix::ffi::OsStrExt;
        use std::os::unix::fs::symlink;

        let dir = tempfile::tempdir().unwrap();
        let non_utf8_dir = dir.path().join(OsStr::from_bytes(b"bad\xffdir"));
        std::fs::create_dir(&non_utf8_dir).unwrap();
        let target = non_utf8_dir.join("book.cbz");
        std::fs::write(&target, b"").unwrap();
        let link = dir.path().join("book-link.cbz");
        symlink(target, &link).unwrap();
        (dir, link)
    }

    #[cfg(unix)]
    #[test]
    fn canonical_identity_falls_back_when_the_canonical_form_is_not_utf8() {
        // macOS filesystems reject invalid-UTF-8 names; the defect is exposed on
        // Unix platforms that permit arbitrary path bytes.
        if cfg!(target_os = "macos") {
            return;
        }
        let (_dir, link) = utf8_symlink_to_non_utf8_target();

        assert_eq!(canonical_identity(&link), link);
    }

    #[cfg(unix)]
    #[test]
    fn a_library_built_through_canonical_identity_always_serializes() {
        // macOS filesystems reject invalid-UTF-8 names; the defect is exposed on
        // Unix platforms that permit arbitrary path bytes.
        if cfg!(target_os = "macos") {
            return;
        }
        let (_dir, link) = utf8_symlink_to_non_utf8_target();
        let mut lib = Library::new();

        assert!(lib
            .add(
                BookId::from_canonical_path(&canonical_identity(&link)).unwrap(),
                display_title(&link, false)
            )
            .is_some());
        let value: serde_json::Value =
            serde_json::from_str(&lib.to_json().expect("UTF-8 identity must serialize")).unwrap();

        assert_eq!(value["books"][0]["path"].as_str(), link.to_str());
    }

    #[test]
    fn new_library_is_empty() {
        let lib = Library::new();
        assert!(lib.books().is_empty());
    }

    #[test]
    fn clear_empty_library_returns_false() {
        let mut lib = Library::new();

        assert!(
            !lib.clear(),
            "clearing an already-empty library changes no persisted state"
        );
        assert!(lib.books().is_empty());
        assert_eq!(lib.last_opened(), None);
    }

    #[test]
    fn clear_non_empty_library_removes_books_and_last_opened() {
        let mut lib = Library::new();
        let first = BookId::new("/manga/a.cbz");
        let second = BookId::new("/manga/b.cbz");
        assert!(lib.add(first.clone(), title(&first)).is_some());
        assert!(lib.add(second.clone(), title(&second)).is_some());
        lib.register_opened(&second, &title(&second), NonZeroUsize::new(24));

        assert!(
            lib.clear(),
            "clearing a populated library changes persisted state"
        );
        assert!(lib.books().is_empty(), "all books are removed");
        assert_eq!(lib.last_opened(), None, "last_opened is cleared");
    }

    #[test]
    fn add_orders_books_by_natural_title() {
        // Non-existent paths are already their caller-supplied identities.
        let mut lib = Library::new();
        assert!(lib
            .add(
                BookId::new("/manga/vol 10.cbz"),
                display_title(Path::new(&"/manga/vol 10.cbz"), false)
            )
            .is_some());
        assert!(lib
            .add(
                BookId::new("/manga/vol 1.cbz"),
                display_title(Path::new(&"/manga/vol 1.cbz"), false)
            )
            .is_some());
        assert!(lib
            .add(
                BookId::new("/manga/vol 2.cbz"),
                display_title(Path::new(&"/manga/vol 2.cbz"), false)
            )
            .is_some());
        let titles: Vec<&str> = lib.books().iter().map(|b| b.title()).collect();
        assert_eq!(
            titles,
            vec!["vol 1", "vol 2", "vol 10"],
            "library order must use natural title sorting"
        );
    }

    #[test]
    fn add_tie_breaks_same_title_by_canonical_path() {
        let dir = tempfile::tempdir().unwrap();
        let a_dir = dir.path().join("a");
        let b_dir = dir.path().join("b");
        std::fs::create_dir(&a_dir).unwrap();
        std::fs::create_dir(&b_dir).unwrap();
        let a = a_dir.join("Same.cbz");
        let b = b_dir.join("Same.cbz");
        std::fs::write(&a, []).unwrap();
        std::fs::write(&b, []).unwrap();
        let a = BookId::from_canonical_path(&a.canonicalize().unwrap()).unwrap();
        let b = BookId::from_canonical_path(&b.canonicalize().unwrap()).unwrap();

        let mut lib = Library::new();
        assert!(lib.add(b.clone(), title(&b)).is_some());
        assert!(lib.add(a.clone(), title(&a)).is_some());

        let paths: Vec<&BookId> = lib.books().iter().map(|book| book.id()).collect();
        assert_eq!(paths, vec![&a, &b]);
    }

    #[test]
    fn add_dedups_by_path_and_returns_false() {
        let mut lib = Library::new();
        assert!(lib
            .add(
                BookId::new("/manga/a.cbz"),
                display_title(Path::new(&"/manga/a.cbz"), false)
            )
            .is_some());
        assert!(
            lib.add(
                BookId::new("/manga/a.cbz"),
                display_title(Path::new(&"/manga/a.cbz"), false)
            )
            .is_none(),
            "adding an existing path must be a no-op returning None"
        );
        assert_eq!(lib.books().len(), 1);
    }

    #[test]
    fn add_returns_stored_canonical_path_then_none_on_duplicate() {
        // A real temp file whose identity is produced caller-side; the first add
        // returns that canonical path and the second (duplicate) is None.
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("Book.cbz");
        std::fs::write(&file, []).unwrap();
        let canonical = BookId::from_canonical_path(&file.canonicalize().unwrap()).unwrap();

        let mut lib = Library::new();
        assert_eq!(
            lib.add(
                BookId::from_canonical_path(&canonical_identity(&file)).unwrap(),
                display_title(&file, false)
            ),
            Some(&canonical),
            "first add must return the stored canonical path"
        );
        assert!(
            lib.add(
                BookId::from_canonical_path(&canonical_identity(&file)).unwrap(),
                display_title(&file, false)
            )
            .is_none(),
            "adding the same path again must return None (duplicate)"
        );
        assert_eq!(lib.books().len(), 1);
    }

    #[test]
    fn add_many_matches_repeated_add() {
        let already_present = BookId::new("/manga/already.cbz");
        let vol10 = BookId::new("/manga/vol 10.cbz");
        let vol2 = BookId::new("/manga/vol 2.cbz");
        let vol1 = BookId::new("/manga/vol 1.cbz");
        let inputs = vec![
            vol10.clone(),
            already_present.clone(),
            vol2.clone(),
            vol2.clone(),
            vol1.clone(),
        ];

        let mut batched = Library::new();
        assert!(batched
            .add(already_present.clone(), title(&already_present))
            .is_some());
        let added = batched.add_many(inputs.iter().map(|id| (id.clone(), title(id))).collect());

        let mut repeated = Library::new();
        assert!(repeated
            .add(already_present.clone(), title(&already_present))
            .is_some());
        let repeated_added: Vec<BookId> = inputs
            .into_iter()
            .filter_map(|path| repeated.add(path.clone(), title(&path)).cloned())
            .collect();

        let batched_paths: Vec<&BookId> = batched.books().iter().map(Book::id).collect();
        let repeated_paths: Vec<&BookId> = repeated.books().iter().map(Book::id).collect();
        assert_eq!(batched_paths, repeated_paths);
        assert_eq!(added, repeated_added);
        assert_eq!(added, vec![vol10, vol2, vol1]);
    }

    #[test]
    fn add_many_sorts_once_into_natural_order() {
        let vol10 = BookId::new("/manga/vol 10.cbz");
        let vol2 = BookId::new("/manga/vol 2.cbz");
        let vol1 = BookId::new("/manga/vol 1.cbz");
        let mut lib = Library::new();

        assert_eq!(
            lib.add_many(
                [&vol10, &vol2, &vol1]
                    .map(|id| (id.clone(), title(id)))
                    .to_vec()
            ),
            vec![vol10.clone(), vol2.clone(), vol1.clone()]
        );

        let paths: Vec<&BookId> = lib.books().iter().map(Book::id).collect();
        assert_eq!(paths, vec![&vol1, &vol2, &vol10]);
    }

    #[test]
    fn add_many_empty_input_is_a_no_op() {
        let existing = BookId::new("/manga/existing.cbz");
        let mut lib = Library::new();
        assert!(lib.add(existing.clone(), title(&existing)).is_some());
        let before = lib.to_json().unwrap();

        assert!(lib.add_many(Vec::new()).is_empty());

        assert_eq!(lib.to_json().unwrap(), before);
        assert_eq!(lib.books()[0].id(), &existing);
    }

    #[test]
    fn add_stores_caller_supplied_canonical_identity() {
        let dir = tempfile::tempdir().unwrap();
        let folder = dir.path().join("Series.1997");
        std::fs::create_dir(&folder).unwrap();
        let canonical =
            BookId::from_canonical_path(&canonical_identity(&folder.join("."))).unwrap();

        let mut lib = Library::new();
        assert!(lib
            .add(canonical.clone(), display_title(&folder, true))
            .is_some());
        assert_eq!(lib.books().len(), 1);
        assert_eq!(lib.books()[0].id(), &canonical);
        assert_eq!(lib.books()[0].title(), "Series.1997");
    }

    #[test]
    fn add_stores_the_given_identity_verbatim() {
        // The aggregate no longer canonicalises: whatever identity the caller
        // supplies is what gets stored, even when the filesystem could resolve it
        // to something shorter.
        let dir = tempfile::tempdir().unwrap();
        let folder = dir.path().join("Series.1997");
        std::fs::create_dir(&folder).unwrap();
        // Existing, resolvable, but deliberately NOT canonical.
        let non_canonical = dir
            .path()
            .join("..")
            .join(dir.path().file_name().expect("tempdir has a leaf name"));
        let non_canonical = non_canonical.join("Series.1997");
        assert_ne!(
            non_canonical,
            non_canonical.canonicalize().unwrap(),
            "the fixture must be genuinely non-canonical"
        );

        let mut lib = Library::new();
        assert!(lib
            .add(
                BookId::from_canonical_path(&non_canonical).unwrap(),
                display_title(&non_canonical, true)
            )
            .is_some());
        assert_eq!(lib.books().len(), 1);
        assert_eq!(
            lib.books()[0].id(),
            &BookId::from_canonical_path(&non_canonical).unwrap(),
            "add must store the caller-supplied identity verbatim"
        );
    }

    #[test]
    fn remove_existing_returns_true_and_drops_book() {
        let mut lib = Library::new();
        let path = BookId::new("/manga/a.cbz");
        assert!(lib.add(path.clone(), title(&path)).is_some());
        assert!(lib.remove(&path));
        assert!(lib.books().is_empty());
    }

    #[test]
    fn remove_absent_returns_false() {
        let mut lib = Library::new();
        assert!(!lib.remove(&BookId::new("/manga/missing.cbz")));
    }

    #[test]
    fn remove_many_removes_multiple_and_reports_them() {
        let mut lib = Library::new();
        for name in ["a.cbz", "b.cbz", "c.cbz"] {
            assert!(lib
                .add(
                    BookId::new(format!("/manga/{name}")),
                    display_title(Path::new(&format!("/manga/{name}")), false)
                )
                .is_some());
        }
        let report = lib.remove_many(&[BookId::new("/manga/a.cbz"), BookId::new("/manga/c.cbz")]);
        assert_eq!(
            report.removed,
            vec![BookId::new("/manga/a.cbz"), BookId::new("/manga/c.cbz")]
        );
        assert!(report.not_found.is_empty());
        let titles: Vec<&str> = lib.books().iter().map(|b| b.title()).collect();
        assert_eq!(titles, vec!["b"], "only the unremoved book survives");
    }

    #[test]
    fn remove_many_preserves_natural_order_of_survivors() {
        // Survivors must stay in natural title order after a bulk removal (the
        // retain keeps relative order, so no re-sort is needed).
        let mut lib = Library::new();
        for name in ["vol 1.cbz", "vol 2.cbz", "vol 10.cbz", "vol 11.cbz"] {
            assert!(lib
                .add(
                    BookId::new(format!("/manga/{name}")),
                    display_title(Path::new(&format!("/manga/{name}")), false)
                )
                .is_some());
        }
        // Remove the two middle volumes; vol 1 and vol 11 must remain in order.
        let report = lib.remove_many(&[
            BookId::new("/manga/vol 2.cbz"),
            BookId::new("/manga/vol 10.cbz"),
        ]);
        assert_eq!(report.removed.len(), 2);
        let titles: Vec<&str> = lib.books().iter().map(|b| b.title()).collect();
        assert_eq!(
            titles,
            vec!["vol 1", "vol 11"],
            "survivors keep natural title order"
        );
    }

    #[test]
    fn remove_many_splits_not_found() {
        let mut lib = Library::new();
        assert!(lib
            .add(
                BookId::new("/manga/a.cbz"),
                display_title(Path::new(&"/manga/a.cbz"), false)
            )
            .is_some());
        let report = lib.remove_many(&[
            BookId::new("/manga/a.cbz"),
            BookId::new("/manga/missing.cbz"),
        ]);
        assert_eq!(report.removed, vec![BookId::new("/manga/a.cbz")]);
        assert_eq!(
            report.not_found,
            vec![BookId::new("/manga/missing.cbz")],
            "an input matching no book is reported, not removed"
        );
        assert!(lib.books().is_empty());
    }

    #[test]
    fn remove_many_empty_input_yields_empty_report() {
        let mut lib = Library::new();
        let book = BookId::new("/manga/a.cbz");
        assert!(lib.add(book.clone(), title(&book)).is_some());
        // Register it as last-opened so we can verify it is preserved.
        lib.register_opened(&book, &title(&book), None);
        let report = lib.remove_many(&[]);
        assert_eq!(report, RemovalReport::default());
        assert!(report.removed.is_empty());
        assert!(report.not_found.is_empty());
        assert_eq!(lib.books().len(), 1, "no input removes nothing");
        assert_eq!(
            lib.last_opened(),
            Some(&book),
            "empty remove_many must not disturb last_opened"
        );
    }

    #[test]
    fn remove_many_counts_duplicate_input_once() {
        let mut lib = Library::new();
        assert!(lib
            .add(
                BookId::new("/manga/a.cbz"),
                display_title(Path::new(&"/manga/a.cbz"), false)
            )
            .is_some());
        // The same path passed twice must be removed once and reported once.
        let report = lib.remove_many(&[BookId::new("/manga/a.cbz"), BookId::new("/manga/a.cbz")]);
        assert_eq!(
            report.removed,
            vec![BookId::new("/manga/a.cbz")],
            "a duplicated input path is counted once"
        );
        assert!(report.not_found.is_empty());
        assert!(lib.books().is_empty());
    }

    #[test]
    fn restore_reinserts_full_books_and_restores_natural_order() {
        // remove_many returns only paths; the rollback primitive must put back
        // WHOLE books (carrying resume_page/page_count/overrides) and re-sort.
        let mut lib = Library::new();
        for name in ["vol 1.cbz", "vol 2.cbz", "vol 10.cbz"] {
            assert!(lib
                .add(
                    BookId::new(format!("/manga/{name}")),
                    display_title(Path::new(&format!("/manga/{name}")), false)
                )
                .is_some());
        }
        // Give the middle volume a non-default reading position before removal.
        assert!(lib.set_resume_page(&BookId::new("/manga/vol 2.cbz"), 7));
        assert!(lib.set_page_count(
            &BookId::new("/manga/vol 2.cbz"),
            NonZeroUsize::new(20).unwrap()
        ));

        // Clone the two outer volumes, remove them, then restore the clones.
        let removed: Vec<Book> = lib
            .books()
            .iter()
            .filter(|b| {
                b.id() == &BookId::new("/manga/vol 1.cbz")
                    || b.id() == &BookId::new("/manga/vol 10.cbz")
            })
            .cloned()
            .collect();
        let report = lib.remove_many(&[
            BookId::new("/manga/vol 1.cbz"),
            BookId::new("/manga/vol 10.cbz"),
        ]);
        assert_eq!(report.removed.len(), 2);
        let titles: Vec<&str> = lib.books().iter().map(|b| b.title()).collect();
        assert_eq!(titles, vec!["vol 2"], "only the unremoved book survives");

        lib.restore(removed, None);
        let titles: Vec<&str> = lib.books().iter().map(|b| b.title()).collect();
        assert_eq!(
            titles,
            vec!["vol 1", "vol 2", "vol 10"],
            "restore re-inserts and restores natural order"
        );
        // The untouched middle volume kept its position; the restored ones are intact.
        assert_eq!(lib.resume_page(&BookId::new("/manga/vol 2.cbz")), 7);
    }

    #[test]
    fn restore_preserves_per_book_data_that_add_would_lose() {
        // The add()-trap: re-adding by path yields a FRESH book (resume_page 0, count None,
        // overrides all-None); restore must re-insert the WHOLE clone, keeping that data.
        let mut lib = Library::new();
        let p = BookId::new("/manga/a.cbz");
        assert!(lib.add(p.clone(), title(&p)).is_some());
        assert!(lib.set_resume_page(&p, 42));
        assert!(lib.set_page_count(&p, NonZeroUsize::new(100).unwrap()));
        let ov = crate::view_override::ViewOverride {
            reading_direction: Some(crate::view_modes::ReadingDirection::Rtl),
            ..crate::view_override::ViewOverride::none()
        };
        assert!(lib.set_overrides(&p, ov));

        let clone: Vec<Book> = lib.books().to_vec();
        assert_eq!(lib.remove_many(std::slice::from_ref(&p)).removed.len(), 1);
        assert!(lib.books().is_empty());

        lib.restore(clone, None);
        assert_eq!(lib.books().len(), 1);
        assert_eq!(
            lib.resume_page(&p),
            42,
            "restore keeps resume_page (add would reset to 0)"
        );
        assert_eq!(lib.books()[0].page_count_opt(), Some(100));
        assert_eq!(
            lib.overrides_for(&p),
            ov,
            "restore keeps overrides (add would reset to all-None)"
        );
    }

    #[test]
    fn restore_is_duplicate_safe() {
        // Restoring a book whose path is already present must NOT duplicate it.
        let mut lib = Library::new();
        let p = BookId::new("/manga/a.cbz");
        assert!(lib.add(p.clone(), title(&p)).is_some());
        let clone: Vec<Book> = lib.books().to_vec();
        // The book is still present; restoring its clone is a no-op (dedup by path).
        lib.restore(clone, None);
        assert_eq!(
            lib.books().len(),
            1,
            "restore must not duplicate an existing path"
        );
    }

    #[test]
    fn restore_empty_input_is_noop() {
        let mut lib = Library::new();
        assert!(lib
            .add(
                BookId::new("/manga/a.cbz"),
                display_title(Path::new(&"/manga/a.cbz"), false)
            )
            .is_some());
        lib.restore(Vec::new(), None);
        assert_eq!(lib.books().len(), 1, "empty restore changes nothing");
    }

    #[test]
    fn restore_round_trips_to_byte_identical_json() {
        // The rollback contract: remove + restore must reproduce the EXACT pre-removal
        // JSON. A non-default override keeps the `overrides` comparison non-vacuous.
        let mut lib = Library::new();
        for name in ["a.cbz", "b.cbz", "c.cbz"] {
            assert!(lib
                .add(
                    BookId::new(format!("/manga/{name}")),
                    display_title(Path::new(&format!("/manga/{name}")), false)
                )
                .is_some());
        }
        assert!(lib.set_resume_page(&BookId::new("/manga/b.cbz"), 9));
        assert!(lib.set_page_count(&BookId::new("/manga/b.cbz"), NonZeroUsize::new(50).unwrap()));
        let ov = crate::view_override::ViewOverride {
            reading_direction: Some(crate::view_modes::ReadingDirection::Rtl),
            ..crate::view_override::ViewOverride::none()
        };
        assert!(lib.set_overrides(&BookId::new("/manga/b.cbz"), ov));
        let before = lib.to_json().unwrap();

        let removed: Vec<Book> = lib
            .books()
            .iter()
            .filter(|b| b.id() != &BookId::new("/manga/a.cbz"))
            .cloned()
            .collect();
        lib.remove_many(&[BookId::new("/manga/b.cbz"), BookId::new("/manga/c.cbz")]);
        lib.restore(removed, None);
        assert_eq!(
            lib.to_json().unwrap(),
            before,
            "remove + restore must be byte-identical"
        );
    }

    #[test]
    fn restore_recovers_last_opened_bookmark() {
        // The Alloy counterexample: read B (last_opened = B), remove B (remove_many
        // clears the bookmark), then a failed save rolls back via restore — which must
        // hand the bookmark back so the "continue reading" target is not silently lost.
        let mut lib = Library::new();
        for name in ["a.cbz", "b.cbz"] {
            assert!(lib
                .add(
                    BookId::new(format!("/manga/{name}")),
                    display_title(Path::new(&format!("/manga/{name}")), false)
                )
                .is_some());
        }
        let b = BookId::new("/manga/b.cbz");
        lib.register_opened(&b, &title(&b), None);
        assert_eq!(lib.last_opened(), Some(&b), "B is the bookmark");

        let removed: Vec<Book> = lib
            .books()
            .iter()
            .filter(|book| book.id() == &b)
            .cloned()
            .collect();
        assert_eq!(lib.remove_many(std::slice::from_ref(&b)).removed.len(), 1);
        assert_eq!(lib.last_opened(), None, "remove_many clears the bookmark");

        lib.restore(removed, Some(b.clone()));
        assert_eq!(lib.books().len(), 2, "both books restored");
        assert_eq!(
            lib.last_opened(),
            Some(&b),
            "restore recovers the last_opened bookmark"
        );
    }

    #[test]
    fn restore_drops_a_stale_bookmark_not_on_the_shelf() {
        // The membership guard (invariant S69): a last_opened that points at no
        // shelved book must be dropped, never re-installed as a dangling bookmark.
        let mut lib = Library::new();
        let a = BookId::new("/manga/a.cbz");
        assert!(lib.add(a.clone(), title(&a)).is_some());
        let removed: Vec<Book> = lib.books().to_vec();
        assert_eq!(lib.remove_many(std::slice::from_ref(&a)).removed.len(), 1);

        // Restore the book but point the bookmark at a book that was never re-inserted.
        lib.restore(removed, Some(BookId::new("/manga/ghost.cbz")));
        assert_eq!(lib.books().len(), 1, "the removed book is back");
        assert_eq!(
            lib.last_opened(),
            None,
            "a bookmark absent from the shelf must not be installed"
        );
    }

    #[test]
    fn last_page_is_zero_for_unknown_path() {
        let lib = Library::new();
        assert_eq!(lib.resume_page(&BookId::new("/manga/missing.cbz")), 0);
    }

    #[test]
    fn set_resume_page_updates_and_round_trips() {
        let mut lib = Library::new();
        let path = BookId::new("/manga/a.cbz");
        assert!(lib.add(path.clone(), title(&path)).is_some());
        assert!(lib.set_resume_page(&path, 42));
        assert_eq!(lib.resume_page(&path), 42);
    }

    #[test]
    fn set_resume_page_false_when_absent() {
        let mut lib = Library::new();
        assert!(!lib.set_resume_page(&BookId::new("/manga/missing.cbz"), 12));
    }

    #[test]
    fn set_resume_page_false_when_unchanged() {
        let mut lib = Library::new();
        let path = BookId::new("/manga/a.cbz");
        assert!(lib.add(path.clone(), title(&path)).is_some());
        assert!(lib.set_resume_page(&path, 3));
        assert!(!lib.set_resume_page(&path, 3));
    }

    #[test]
    fn page_count_opt_is_none_for_fresh_book() {
        let book = book_from_path(Path::new("/manga/a.cbz"), false);
        assert_eq!(book.page_count_opt(), None);
    }

    #[test]
    fn set_page_count_updates_and_returns_true() {
        let mut lib = Library::new();
        let path = BookId::new("/manga/a.cbz");
        assert!(lib.add(path.clone(), title(&path)).is_some());
        assert!(lib.set_page_count(&path, NonZeroUsize::new(42).unwrap()));
        assert_eq!(lib.books()[0].page_count_opt(), Some(42));
    }

    #[test]
    fn set_page_count_false_when_absent() {
        let mut lib = Library::new();
        assert!(!lib.set_page_count(
            &BookId::new("/manga/missing.cbz"),
            NonZeroUsize::new(12).unwrap()
        ));
    }

    #[test]
    fn set_page_count_false_when_unchanged() {
        let mut lib = Library::new();
        let path = BookId::new("/manga/a.cbz");
        assert!(lib.add(path.clone(), title(&path)).is_some());
        assert!(lib.set_page_count(&path, NonZeroUsize::new(3).unwrap()));
        assert!(!lib.set_page_count(&path, NonZeroUsize::new(3).unwrap()));
    }

    #[test]
    fn set_page_count_repairs_a_stale_resume_page() {
        // A book read to page 41 whose archive later shrank to 5 pages: writing the
        // new count must also pull the stored resume back into the book.
        let mut lib = Library::new();
        let path = BookId::new("/manga/a.cbz");
        assert!(lib.add(path.clone(), title(&path)).is_some());
        assert!(lib.set_resume_page(&path, 40));
        assert!(
            lib.set_page_count(&path, NonZeroUsize::new(5).unwrap()),
            "the count write changed something, so the caller must save"
        );
        assert_eq!(
            lib.resume_page(&path),
            4,
            "resume clamped to the last index"
        );
        assert_eq!(lib.books()[0].progress().current(), 5);
    }

    #[test]
    fn set_page_count_reports_a_resume_only_repair() {
        let mut lib = Library::new();
        let path = BookId::new("/manga/a.cbz");
        let count = NonZeroUsize::new(5).unwrap();
        assert!(lib.add(path.clone(), title(&path)).is_some());
        assert!(lib.set_resume_page(&path, 9));
        assert!(lib.set_page_count(&path, count));
        assert_eq!(lib.resume_page(&path), 4);
        assert!(!lib.set_page_count(&path, count));
    }

    #[test]
    fn set_resume_page_clamps_into_a_known_total() {
        let mut lib = Library::new();
        let path = BookId::new("/manga/a.cbz");
        assert!(lib.add(path.clone(), title(&path)).is_some());
        assert!(lib.set_page_count(&path, NonZeroUsize::new(5).unwrap()));
        assert!(lib.set_resume_page(&path, 9));
        assert_eq!(lib.resume_page(&path), 4);
        assert!(!lib.set_resume_page(&path, 4));
        assert!(!lib.set_resume_page(&path, 9));
    }

    #[test]
    fn stale_pair_in_json_is_clamped_on_load() {
        let json = r#"{"path":"a.cbz","title":"a","last_page":40,"page_count":5}"#;
        let book: Book = serde_json::from_str(json).unwrap();
        assert_eq!(book.resume_page(), 4);
        assert_eq!(serde_json::to_value(book).unwrap()["last_page"], 4);
    }

    #[test]
    fn book_json_shape_is_byte_identical() {
        let mut lib = Library::new();
        let path = BookId::new("a.cbz");
        assert!(lib.add(path.clone(), title(&path)).is_some());
        assert!(lib.set_resume_page(&path, 3));
        let unknown = r#"{
  "version": 1,
  "books": [
    {
      "path": "a.cbz",
      "title": "a",
      "last_page": 3,
      "page_count": 0
    }
  ]
}"#;
        assert_eq!(lib.to_json().unwrap(), unknown);
        assert!(lib.set_page_count(&path, NonZeroUsize::new(10).unwrap()));
        let known = unknown.replace(r#""page_count": 0"#, r#""page_count": 10"#);
        assert_eq!(lib.to_json().unwrap(), known);
    }

    #[test]
    fn book_without_page_count_field_deserializes_to_unknown() {
        // Backward compat: an older `Book` JSON predating the `page_count` field must
        // still deserialize, the missing field defaulting to unknown (`None`).
        let value = serde_json::json!({
            "path": "/manga/a.cbz",
            "title": "a",
            "last_page": 5,
        });
        let book: Book = serde_json::from_value(value).unwrap();
        assert_eq!(book.id(), &BookId::new("/manga/a.cbz"));
        assert_eq!(book.resume_page(), 5);
        assert_eq!(book.page_count_opt(), None);
    }

    #[test]
    fn progress_is_at_start_for_freshly_added_book() {
        let mut lib = Library::new();
        let path = BookId::new("/manga/a.cbz");
        assert!(lib.add(path.clone(), title(&path)).is_some());
        let p = lib.books()[0].progress();
        assert!(p.is_at_start(), "freshly added book must be at the start");
        assert_eq!(p.last_viewed(), 0);
        assert_eq!(p.total(), None);
    }

    #[test]
    fn progress_reflects_last_page_and_page_count() {
        let mut lib = Library::new();
        let path = BookId::new("/manga/a.cbz");
        assert!(lib.add(path.clone(), title(&path)).is_some());
        assert!(lib.set_resume_page(&path, 3));
        assert!(lib.set_page_count(&path, NonZeroUsize::new(10).unwrap()));
        let p = lib.books()[0].progress();
        assert_eq!(p.last_viewed(), 3);
        assert_eq!(p.total(), Some(10));
        assert_eq!(p.current(), 4);
        let expected: f32 = 3.0 / 9.0;
        assert!(
            (p.fraction() - expected).abs() < 1e-6,
            "fraction should be 3/9, got {}",
            p.fraction()
        );
    }

    #[test]
    fn register_opened_adds_fresh_book_and_back_fills_count() {
        let mut lib = Library::new();
        let path = BookId::new("/manga/a.cbz");
        let reg = lib.register_opened(&path, &title(&path), NonZeroUsize::new(10));
        assert_eq!(lib.books().len(), 1, "fresh open must add the book");
        assert!(reg.count_changed, "0 -> 10 must report a count change");
        assert!(reg.resume.is_at_start(), "fresh book resumes at the start");
        assert_eq!(reg.resume.last_viewed(), 0);
        assert_eq!(reg.resume.total(), Some(10));
    }

    #[test]
    fn register_opened_is_idempotent_and_reports_no_change_when_count_unchanged() {
        let mut lib = Library::new();
        let path = BookId::new("/manga/a.cbz");
        lib.register_opened(&path, &title(&path), NonZeroUsize::new(10));
        let reg = lib.register_opened(&path, &title(&path), NonZeroUsize::new(10));
        assert_eq!(lib.books().len(), 1, "re-opening must not duplicate");
        assert!(
            !reg.count_changed,
            "an unchanged count must report no change"
        );
    }

    #[test]
    fn register_opened_skips_back_fill_when_count_unknown() {
        // `None` means the total is unknown: the `is_some_and` guard skips set_page_count,
        // so the stored count stays unknown and surfaces as `total() == None`.
        let mut lib = Library::new();
        let path = BookId::new("/manga/b.cbz");
        let reg = lib.register_opened(&path, &title(&path), None);
        assert_eq!(lib.books().len(), 1, "the book is still added");
        assert!(
            !reg.count_changed,
            "an unknown count must not change the count"
        );
        assert_eq!(reg.resume.total(), None, "count stays unknown");
    }

    #[test]
    fn register_opened_resumes_at_recorded_last_page() {
        let mut lib = Library::new();
        let path = BookId::new("/manga/a.cbz");
        assert!(lib.add(path.clone(), title(&path)).is_some());
        assert!(lib.set_resume_page(&path, 5));
        let reg = lib.register_opened(&path, &title(&path), NonZeroUsize::new(10));
        assert_eq!(
            reg.resume.last_viewed(),
            5,
            "resume reflects the recorded page"
        );
        assert_eq!(reg.resume.current(), 6, "current is last_viewed + 1");
    }

    #[test]
    fn register_opened_twice_preserves_recorded_resume_page() {
        // Re-opening a book must NOT reset its resume position: the idempotent
        // add inside register_opened keeps the existing entry (and its resume_page).
        // The resume is deliberately IN RANGE for the 10-page count — an out-of-range
        // one is now repaired by `set_page_count`, which would test the clamp rather
        // than the preservation this case is about.
        let mut lib = Library::new();
        let p = BookId::new("/manga/a.cbz");
        lib.register_opened(&p, &title(&p), NonZeroUsize::new(10));
        assert!(lib.set_resume_page(&p, 4));
        let reg = lib.register_opened(&p, &title(&p), NonZeroUsize::new(10));
        assert_eq!(lib.books().len(), 1, "re-open must not duplicate the book");
        assert_eq!(
            reg.resume.last_viewed(),
            4,
            "re-open must preserve resume_page"
        );
        assert!(!reg.count_changed, "same count on re-open is no change");
    }

    #[test]
    fn register_opened_reports_count_change_on_new_nonzero_count() {
        // A legitimately changed (non-zero -> different non-zero) page count is
        // reported as count_changed, flowing set_page_count's "did it move" branch.
        let mut lib = Library::new();
        let p = BookId::new("/manga/a.cbz");
        assert!(
            lib.register_opened(&p, &title(&p), NonZeroUsize::new(10))
                .count_changed
        );
        let reg = lib.register_opened(&p, &title(&p), NonZeroUsize::new(12));
        assert!(reg.count_changed, "10 -> 12 is a real change");
        assert_eq!(reg.resume.total(), Some(12));
    }

    #[test]
    fn register_opened_with_caller_supplied_identity_keeps_invariant() {
        // Invariant: register_opened always sets last_opened to the stored book when the
        // caller supplies the identity for a path containing a `..` detour.
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("Book.cbz");
        std::fs::write(&file, []).unwrap();

        // Build a non-canonical path: enter a subdirectory and navigate back up.
        // If the tempdir has no subdirectory yet, create one purely for the detour.
        let sub = dir.path().join("sub");
        std::fs::create_dir(&sub).unwrap();
        let non_canonical = sub.join("..").join("Book.cbz");
        let canonical = BookId::from_canonical_path(&canonical_identity(&non_canonical)).unwrap();

        let mut lib = Library::new();
        lib.register_opened(&canonical, &title(&canonical), None);

        // last_opened is set, and its path is a book actually in the shelf.
        let lo = lib
            .last_opened()
            .expect("register_opened must always set last_opened")
            .clone();
        assert!(
            lib.books().iter().any(|b| b.id() == &lo),
            "last_opened must point at a book actually in the shelf"
        );
        assert_eq!(
            lo, canonical,
            "last_opened must equal the caller-supplied stored identity"
        );
    }

    #[test]
    fn new_book_has_empty_overrides() {
        let book = book_from_path(Path::new("/manga/a.cbz"), false);
        assert!(book.overrides().is_empty());
    }

    #[test]
    fn set_overrides_updates_and_reports_change() {
        let mut lib = Library::new();
        let p = BookId::new("/manga/a.cbz");
        assert!(lib.add(p.clone(), title(&p)).is_some());
        let ov = crate::view_override::ViewOverride {
            reading_direction: Some(crate::view_modes::ReadingDirection::Rtl),
            ..crate::view_override::ViewOverride::none()
        };
        // First set changes the value -> true.
        assert!(lib.set_overrides(&p, ov));
        assert_eq!(lib.overrides_for(&p), ov);
        // Setting the same value again is a no-op -> false (mirrors set_resume_page).
        assert!(!lib.set_overrides(&p, ov));
    }

    #[test]
    fn set_overrides_clear_to_none_reports_change_and_empties() {
        let mut lib = Library::new();
        let p = BookId::new("/manga/a.cbz");
        assert!(lib.add(p.clone(), title(&p)).is_some());
        let ov = crate::view_override::ViewOverride {
            reading_direction: Some(crate::view_modes::ReadingDirection::Rtl),
            ..crate::view_override::ViewOverride::none()
        };
        assert!(lib.set_overrides(&p, ov));
        // Clearing back to none() is a real change -> true, and leaves it empty.
        assert!(lib.set_overrides(&p, crate::view_override::ViewOverride::none()));
        assert!(lib.overrides_for(&p).is_empty());
    }

    #[test]
    fn set_overrides_false_when_path_absent() {
        let mut lib = Library::new();
        let ov = crate::view_override::ViewOverride::none();
        assert!(!lib.set_overrides(&BookId::new("/manga/missing.cbz"), ov));
    }

    #[test]
    fn overrides_for_absent_path_is_empty() {
        let lib = Library::new();
        assert!(lib
            .overrides_for(&BookId::new("/manga/missing.cbz"))
            .is_empty());
    }

    // --- last_opened tests ---

    #[test]
    fn last_opened_is_none_for_fresh_library() {
        let lib = Library::new();
        assert_eq!(lib.last_opened(), None);
    }

    #[test]
    fn register_opened_sets_last_opened_on_fresh_book() {
        let mut lib = Library::new();
        let path = BookId::new("/manga/a.cbz");
        lib.register_opened(&path, &title(&path), None);
        assert_eq!(lib.last_opened(), Some(&BookId::new("/manga/a.cbz")));
    }

    #[test]
    fn register_opened_updates_last_opened_on_reopen() {
        // Opening a different book after the first one replaces last_opened.
        let mut lib = Library::new();
        let a = BookId::new("/manga/a.cbz");
        let b = BookId::new("/manga/b.cbz");
        lib.register_opened(&a, &title(&a), None);
        lib.register_opened(&b, &title(&b), None);
        assert_eq!(lib.last_opened(), Some(&BookId::new("/manga/b.cbz")));
    }

    #[test]
    fn register_opened_same_book_twice_preserves_last_opened() {
        // Re-opening the same book keeps last_opened pointing at it.
        let mut lib = Library::new();
        let path = BookId::new("/manga/a.cbz");
        lib.register_opened(&path, &title(&path), None);
        lib.register_opened(&path, &title(&path), None);
        assert_eq!(lib.last_opened(), Some(&BookId::new("/manga/a.cbz")));
    }

    #[test]
    fn remove_last_opened_book_clears_last_opened() {
        let mut lib = Library::new();
        let path = BookId::new("/manga/a.cbz");
        lib.register_opened(&path, &title(&path), None);
        assert!(lib.remove(&path));
        assert_eq!(lib.last_opened(), None);
    }

    #[test]
    fn remove_different_book_preserves_last_opened() {
        let mut lib = Library::new();
        let a = BookId::new("/manga/a.cbz");
        let b = BookId::new("/manga/b.cbz");
        lib.register_opened(&a, &title(&a), None);
        lib.add(b.clone(), title(&b));
        assert!(lib.remove(&b));
        assert_eq!(
            lib.last_opened(),
            Some(&BookId::new("/manga/a.cbz")),
            "removing a different book must not clear last_opened"
        );
    }

    #[test]
    fn remove_many_containing_last_opened_clears_it() {
        let mut lib = Library::new();
        let a = BookId::new("/manga/a.cbz");
        let b = BookId::new("/manga/b.cbz");
        lib.register_opened(&a, &title(&a), None);
        lib.add(b.clone(), title(&b));
        lib.remove_many(&[a.clone(), b.clone()]);
        assert_eq!(lib.last_opened(), None);
    }

    #[test]
    fn remove_many_not_containing_last_opened_preserves_it() {
        let mut lib = Library::new();
        let a = BookId::new("/manga/a.cbz");
        let b = BookId::new("/manga/b.cbz");
        lib.register_opened(&a, &title(&a), None);
        lib.add(b.clone(), title(&b));
        lib.remove_many(std::slice::from_ref(&b));
        assert_eq!(
            lib.last_opened(),
            Some(&BookId::new("/manga/a.cbz")),
            "removing an unrelated book must not clear last_opened"
        );
    }

    // --- bookmark tests (resume target = last_opened only when shelved) ---

    #[test]
    fn bookmark_is_some_when_last_opened_is_shelved() {
        let mut lib = Library::new();
        let path = BookId::new("/manga/a.cbz");
        lib.register_opened(&path, &title(&path), None);
        assert_eq!(
            lib.bookmark(),
            Some(&BookId::new("/manga/a.cbz")),
            "bookmark resolves to a shelved last_opened"
        );
    }

    #[test]
    fn bookmark_is_none_when_last_opened_is_none() {
        let mut lib = Library::new();
        // A shelved book but nothing opened yet: no resume target.
        assert!(lib
            .add(
                BookId::new("/manga/a.cbz"),
                display_title(Path::new(&"/manga/a.cbz"), false)
            )
            .is_some());
        assert_eq!(lib.last_opened(), None);
        assert_eq!(lib.bookmark(), None, "no last_opened means no bookmark");
    }

    #[test]
    fn bookmark_is_none_for_orphan_last_opened() {
        // last_opened points at a path NOT in books, built via RAW serde deserialization
        // (not from_json, which normalize() would clear), so bookmark() guards membership.
        let json = serde_json::json!({
            "books": [{"path": "/manga/a.cbz", "title": "a", "last_page": 0, "page_count": 0}],
            "last_opened": "/manga/gone.cbz"
        })
        .to_string();
        let lib: Library = serde_json::from_str(&json).unwrap();
        assert_eq!(
            lib.last_opened(),
            Some(&BookId::new("/manga/gone.cbz")),
            "raw deserialization preserves the orphan (no normalize)"
        );
        assert_eq!(
            lib.bookmark(),
            None,
            "bookmark must reject a last_opened that is not a shelved book"
        );
    }

    #[test]
    fn normalize_clears_orphan_last_opened() {
        // Build a library via serde with a last_opened that points at a path
        // NOT present in books, then confirm normalize() resets it to None.
        let json = serde_json::json!({
            "version": 1,
            "books": [{"path": "/manga/a.cbz", "title": "a", "last_page": 0, "page_count": 0}],
            "last_opened": "/manga/gone.cbz"
        })
        .to_string();
        let lib = Library::from_json(&json).unwrap();
        assert_eq!(
            lib.last_opened(),
            None,
            "normalize must clear an orphan last_opened"
        );
    }

    #[test]
    fn normalize_keeps_valid_last_opened() {
        // A valid last_opened (path is in books) must survive normalize().
        let json = serde_json::json!({
            "version": 1,
            "books": [{"path": "/manga/a.cbz", "title": "a", "last_page": 0, "page_count": 0}],
            "last_opened": "/manga/a.cbz"
        })
        .to_string();
        let lib = Library::from_json(&json).unwrap();
        assert_eq!(
            lib.last_opened(),
            Some(&BookId::new("/manga/a.cbz")),
            "normalize must keep a valid last_opened"
        );
    }

    // recanonicalize_and_merge tests: each builds a `library.json` with path spellings
    // crafted directly (bypassing add-time canonicalization) so two entries name one file.

    /// Build a non-canonical spelling of `canonical` by routing through a `..`
    /// detour (`<dir>/sub/../<name>`), creating the `sub` directory so the path
    /// resolves. A `..` component is NOT folded away by `Path` comparison (unlike
    /// `.`), so the result is genuinely distinct from `canonical` AND canonicalizes
    /// back to it — faithfully reproducing the divergent-spelling bug (a plain `.`
    /// segment would compare equal and the add-time dedup would already catch it).
    fn detour_spelling(canonical: &Path) -> PathBuf {
        let parent = canonical.parent().unwrap();
        let sub = parent.join("sub");
        std::fs::create_dir_all(&sub).unwrap();
        sub.join("..").join(canonical.file_name().unwrap())
    }

    #[test]
    fn recanonicalize_merges_two_spellings_of_same_file() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("Book.cbz");
        std::fs::write(&file, []).unwrap();
        let canonical = file.canonicalize().unwrap();
        let non_canonical = detour_spelling(&canonical);
        assert_ne!(
            canonical, non_canonical,
            "the two spellings must differ as strings"
        );

        // Two entries naming the SAME file, fields set so the merge policy is non-vacuous:
        // max resume_page + first Some page_count from member[1]; override + title from member[0].
        let json = serde_json::json!({
            "version": 1,
            "books": [
                {"path": canonical.to_str().unwrap(), "title": "Book", "last_page": 3,
                 "page_count": 0, "overrides": {"reading_direction": "rtl"}},
                {"path": non_canonical.to_str().unwrap(), "title": "Other", "last_page": 7,
                 "page_count": 20},
            ]
        })
        .to_string();

        let lib = Library::from_json(&json).unwrap();

        assert_eq!(
            lib.books().len(),
            1,
            "two spellings of one file merge into a single book"
        );
        let book = &lib.books()[0];
        assert_eq!(
            book.id(),
            &BookId::from_canonical_path(&canonical).unwrap(),
            "survivor path is the canonical key"
        );
        assert_eq!(
            book.resume_page(),
            7,
            "resume_page is the max across members"
        );
        assert_eq!(
            book.page_count_opt(),
            Some(20),
            "page_count is the first Some in vec order"
        );
        assert_eq!(
            book.title(),
            "Book",
            "title is the first member's title (earliest-added identity)"
        );
        assert_eq!(
            book.overrides().reading_direction,
            Some(crate::view_modes::ReadingDirection::Rtl),
            "overrides come from the first non-empty member"
        );
    }

    #[test]
    fn merge_group_clamps_the_survivor_position() {
        // The merge takes `resume_page` as the MAX and `page_count` as the first
        // `Some`, so the two fields can come from DIFFERENT members and pair a large
        // position with a small total. The survivor must be repaired.
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("Book.cbz");
        std::fs::write(&file, []).unwrap();
        let canonical = file.canonicalize().unwrap();
        let non_canonical = detour_spelling(&canonical);

        let json = serde_json::json!({
            "version": 1,
            "books": [
                {"path": canonical.to_str().unwrap(), "title": "Book", "last_page": 0,
                 "page_count": 5},
                {"path": non_canonical.to_str().unwrap(), "title": "Other", "last_page": 40,
                 "page_count": 0},
            ]
        })
        .to_string();

        let lib = Library::from_json(&json).unwrap();

        assert_eq!(lib.books().len(), 1, "the two spellings merge");
        let book = &lib.books()[0];
        assert_eq!(book.page_count_opt(), Some(5), "count is the first Some");
        assert_eq!(
            book.resume_page(),
            4,
            "the max resume is clamped into the surviving page count"
        );
    }

    #[test]
    fn recanonicalize_upgrades_single_noncanonical_path() {
        // A lone entry whose stored path is non-canonical but now resolves must
        // have its path upgraded to canonical on load (group size 1, no merge).
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("Book.cbz");
        std::fs::write(&file, []).unwrap();
        let canonical = file.canonicalize().unwrap();
        let non_canonical = detour_spelling(&canonical);

        let json = serde_json::json!({
            "version": 1,
            "books": [
                {"path": non_canonical.to_str().unwrap(), "title": "Book", "last_page": 5,
                 "page_count": 0},
            ]
        })
        .to_string();

        let lib = Library::from_json(&json).unwrap();

        assert_eq!(lib.books().len(), 1);
        assert_eq!(
            lib.books()[0].id(),
            &BookId::from_canonical_path(&canonical).unwrap(),
            "a resolvable non-canonical path is upgraded to canonical on load"
        );
        assert_eq!(
            lib.books()[0].resume_page(),
            5,
            "the book's data survives the upgrade"
        );
    }

    #[test]
    fn recanonicalize_preserves_missing_path_unchanged() {
        // A path that does not resolve (canonicalize fails) must be kept verbatim —
        // neither dropped nor merged. Regression guard for the missing-file case.
        let raw = "/manga/definitely/missing/Book.cbz";
        let json = serde_json::json!({
            "version": 1,
            "books": [
                {"path": raw, "title": "Book", "last_page": 9, "page_count": 0},
            ]
        })
        .to_string();

        let lib = Library::from_json(&json).unwrap();

        assert_eq!(lib.books().len(), 1, "a missing-path book is kept");
        assert_eq!(
            lib.books()[0].id(),
            &BookId::new(raw),
            "the raw path is preserved unchanged"
        );
        assert_eq!(lib.books()[0].resume_page(), 9);
    }

    #[test]
    fn recanonicalize_repoints_last_opened_to_survivor() {
        // last_opened spelled as the merged-away path must be repointed to the surviving
        // canonical path; without the repoint, normalize's orphan-clear would reset it to None.
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("Book.cbz");
        std::fs::write(&file, []).unwrap();
        let canonical = file.canonicalize().unwrap();
        let non_canonical = detour_spelling(&canonical);

        let json = serde_json::json!({
            "version": 1,
            "books": [
                {"path": canonical.to_str().unwrap(), "title": "Book", "last_page": 0,
                 "page_count": 0},
                {"path": non_canonical.to_str().unwrap(), "title": "Book", "last_page": 0,
                 "page_count": 0},
            ],
            "last_opened": non_canonical.to_str().unwrap(),
        })
        .to_string();

        let lib = Library::from_json(&json).unwrap();

        assert_eq!(
            lib.books().len(),
            1,
            "the duplicate spelling is merged away"
        );
        assert_eq!(
            lib.last_opened(),
            Some(&BookId::from_canonical_path(&canonical).unwrap()),
            "last_opened is repointed to the surviving canonical path"
        );
    }

    #[test]
    fn recanonicalize_keeps_books_in_natural_order() {
        // After re-canonicalization/merge the shelf must still obey book_order. Missing
        // paths are kept verbatim, so this also confirms the sort runs over survivors.
        let json = serde_json::json!({
            "version": 1,
            "books": [
                {"path": "/manga/vol 10.cbz", "title": "vol 10", "last_page": 0, "page_count": 0},
                {"path": "/manga/vol 1.cbz", "title": "vol 1", "last_page": 0, "page_count": 0},
                {"path": "/manga/vol 2.cbz", "title": "vol 2", "last_page": 0, "page_count": 0},
            ]
        })
        .to_string();

        let lib = Library::from_json(&json).unwrap();

        let titles: Vec<&str> = lib.books().iter().map(|b| b.title()).collect();
        assert_eq!(
            titles,
            vec!["vol 1", "vol 2", "vol 10"],
            "post-merge books obey natural title order"
        );
    }
}
