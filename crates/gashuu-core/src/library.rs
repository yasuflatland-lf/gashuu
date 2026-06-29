//! Library domain model: a naturally ordered, de-duplicated shelf of books.
//!
//! Headless (no slint, no tracing). Identity is the canonical filesystem path;
//! availability is derived (never stored). Persistence lives in `library_store`.

use crate::reading_progress::ReadingProgress;
use crate::view_override::ViewOverride;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeSet, HashMap};
use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};

/// One book in the shelf. Identity is the canonical filesystem path.
///
/// Carries display data only: the path, a derived `title` (file stem for a file,
/// directory name for a folder), and the leading page index of the last-viewed
/// spread. The book kind (folder / archive) is resolved at open time by
/// `ArchiveLoader` and is deliberately NOT persisted. Availability (whether the
/// path still resolves) is derived at render time, never stored.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Book {
    path: PathBuf,
    title: String,
    #[serde(default)]
    last_page: usize,
    /// Total page count, cached at open time. The unknown state is expressed in
    /// the type as `None` (rather than a magic `0`): the domain only ever holds a
    /// positive count, so an absent measurement is `None`, never `Some(0)`. The
    /// on-disk shape stays a bare `usize` for byte-compatibility — `0` encodes
    /// `None` and any positive count encodes `Some(n)` — via [`page_count_serde`].
    #[serde(default, with = "page_count_serde")]
    page_count: Option<NonZeroUsize>,
    /// Per-book view preference overrides. `None` fields inherit the global
    /// `Settings`. `skip_serializing_if` keeps an all-None override out of the
    /// JSON entirely, so a book with no overrides serializes byte-identically to
    /// the pre-feature shape (and `#[serde(default)]` loads old files as empty).
    #[serde(default, skip_serializing_if = "ViewOverride::is_empty")]
    overrides: ViewOverride,
}

/// Serde shim for [`Book::page_count`]: the in-memory `Option<NonZeroUsize>` is
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

/// Derive a book's display title from its path. For a folder the title is the
/// directory name; for a file it is the file stem (e.g. `Cool Title` from
/// `Cool Title.cbz`). Either source is rejected when empty, and the lossy full
/// path string is the fallback so the title is never empty. This is the single
/// home of the title rule; `Book::from_path` (and, in later waves, the UI
/// replicas) delegate to it.
pub fn display_title(path: &Path) -> String {
    if path.is_dir() {
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

impl Book {
    /// Build a `Book` from a path, deriving the display title via
    /// [`display_title`] (file stem for a file, directory name for a folder,
    /// lossy full path as a never-empty fallback).
    pub(crate) fn from_path(path: PathBuf) -> Self {
        let title = display_title(&path);
        Self {
            path,
            title,
            last_page: 0,
            page_count: None,
            overrides: ViewOverride::none(),
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

    /// Total page count cached at open time, or `None` when unknown. The field is
    /// already `Option<NonZeroUsize>`; this accessor just unwraps the inner count
    /// to a plain `usize` for callers that don't need the non-zero guarantee.
    pub fn page_count_opt(&self) -> Option<usize> {
        self.page_count.map(NonZeroUsize::get)
    }

    /// This book's reading progress as a value object: the last-viewed leading
    /// page index together with the cached total page count (`None` when the
    /// total is unknown). The `current` / `fraction` derivation lives on
    /// `ReadingProgress`, not at the call sites.
    pub fn progress(&self) -> ReadingProgress {
        ReadingProgress::new(self.last_page, self.page_count_opt())
    }

    /// This book's view preference overrides (all-None when it inherits global).
    pub fn overrides(&self) -> ViewOverride {
        self.overrides
    }
}

fn book_order(a: &Book, b: &Book) -> std::cmp::Ordering {
    crate::ordering::natural_cmp(a.title(), b.title())
        .then_with(|| a.path().as_os_str().cmp(b.path().as_os_str()))
}

/// Collapse one resolution group (books resolving to the same `key`) into a single
/// survivor. The first member is the base — it keeps its `title` (the earliest-added
/// identity) — and the rest fold in under the deterministic merge policy: `last_page`
/// takes the max, `page_count` the first `Some` in vec order, `overrides` the first
/// non-empty in vec order. The survivor's `path` is finally set to `key`, which
/// re-canonicalizes a now-resolvable raw path even for a group of one. `members`
/// is non-empty by construction (a group is only created when a member is pushed).
fn merge_group(key: PathBuf, members: Vec<Book>) -> Book {
    let mut iter = members.into_iter();
    let mut survivor = iter
        .next()
        .expect("a resolution group has at least one member");
    for member in iter {
        survivor.last_page = survivor.last_page.max(member.last_page);
        if survivor.page_count.is_none() {
            survivor.page_count = member.page_count;
        }
        if survivor.overrides.is_empty() {
            survivor.overrides = member.overrides;
        }
    }
    survivor.path = key;
    survivor
}

/// An ordered, de-duplicated shelf of books. Natural title order is the carousel
/// order, with canonical path as the deterministic tie-break. Identity / dedup
/// is on the canonical path (best-effort canonicalized at `add` time). Mirrors
/// `Settings::push_recent` discipline: one place owns the invariants.
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Library {
    #[serde(default)]
    books: Vec<Book>,
    /// The canonical path of the most recently opened book, or `None` when no
    /// book has been opened yet. Invariant: if `Some`, the path is present in
    /// `books`. Maintained by: `register_opened` (sets from the stored book so
    /// membership holds by construction), `remove`/`remove_many` (clear when
    /// the pointed-at book is removed), `normalize()` (clears an orphan on
    /// load).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    last_opened: Option<PathBuf>,
}

/// Outcome of [`Library::remove_many`]: the canonical paths actually removed and
/// the inputs that matched no stored book. A pure value report — never a bare
/// count, so the caller can both surface "removed N" feedback AND distinguish a
/// stale/already-gone selection (`not_found`) from a real removal. The two vecs
/// partition the de-duplicated input: a path appears in exactly one of them, and
/// a duplicated input path is counted once.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct RemovalReport {
    /// The paths that matched a stored book and were unregistered.
    pub removed: Vec<PathBuf>,
    /// The input paths that matched no stored book (already gone / never present).
    pub not_found: Vec<PathBuf>,
}

/// Outcome of [`Library::register_opened`]: where to resume reading and whether the
/// stored page count changed (so the caller can decide whether to rebuild the
/// carousel). A pure value — no I/O implied.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct OpenRegistration {
    /// The position to resume at (the book's recorded `ReadingProgress`).
    pub resume: ReadingProgress,
    /// Whether the stored page count actually changed during registration.
    pub count_changed: bool,
}

impl Library {
    /// An empty library.
    pub fn new() -> Self {
        Self::default()
    }

    /// The books in natural title (carousel) order, with canonical path tie-break.
    pub fn books(&self) -> &[Book] {
        &self.books
    }

    /// The canonical path of the most recently opened book, or `None` when no
    /// book has been opened yet. Invariant: if `Some`, the path is present in
    /// [`books()`](Library::books) (see the `last_opened` field for how it is
    /// maintained).
    pub fn last_opened(&self) -> Option<&Path> {
        self.last_opened.as_deref()
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

    /// The resume target: `last_opened` only when that path is still a shelved
    /// book. This is the single home of the "resume target must be a shelved
    /// book" rule, so callers need not re-derive it by scanning `books`. The
    /// `last_opened` invariant already guarantees membership for a library built
    /// through the public API, but this guards a `Library` reached by a route
    /// that bypasses `normalize` (e.g. raw deserialization), returning `None`
    /// rather than a dangling bookmark.
    pub fn bookmark(&self) -> Option<&Path> {
        let last_opened = self.last_opened()?;
        self.books
            .iter()
            .any(|b| b.path() == last_opened)
            .then_some(last_opened)
    }

    /// Add `path` to the shelf. Canonicalizes best-effort (falling back to the
    /// path verbatim when canonicalization fails - e.g. a missing file), derives
    /// the title, and de-duplicates by canonical path. Returns `Some` with the
    /// canonical path of the newly stored book, or `None` if the path was already
    /// present (duplicate, no-op).
    pub fn add(&mut self, path: PathBuf) -> Option<&Path> {
        let (canonical, added) = self.add_canonical(path);
        // Preserve the public contract: `Some` only for a newly stored book,
        // `None` for an already-present duplicate.
        added.then(|| {
            self.books
                .iter()
                .find(|b| b.path() == canonical)
                .map(Book::path)
                .expect("just-added book must be present")
        })
    }

    /// Canonicalize `path` and ensure it is on the shelf, returning the stored
    /// canonical identity together with whether this call newly added it. The
    /// returned `PathBuf` is the shelf's identity for the book in BOTH cases —
    /// newly added or already present — so a caller that needs to look the book
    /// up after a subsequent `&mut self` call (e.g. `register_opened`) has an
    /// owned key that always matches a stored entry. Internal seam shared by
    /// `add`; not part of the public surface.
    fn add_canonical(&mut self, path: PathBuf) -> (PathBuf, bool) {
        let canonical = path.canonicalize().unwrap_or(path);
        if self.books.iter().any(|b| b.path() == canonical) {
            return (canonical, false);
        }
        self.books.push(Book::from_path(canonical.clone()));
        self.books.sort_by(book_order);
        (canonical, true)
    }

    /// Re-canonicalize stored book paths and merge entries that resolve to the
    /// same physical file into one. This self-heals a `library.json` written
    /// before book identity was the add-time canonical path: a book added while
    /// its file was missing keeps a raw/relative/`..`-bearing path, so when the
    /// same file is later added once it resolves, the two divergent spellings end
    /// up as two `Book` entries for one file with split reading progress.
    ///
    /// Unlike [`normalize`](Library::normalize) this performs filesystem I/O (the
    /// per-book `canonicalize`, the same touch `add` and `book_is_available`
    /// already make), so it is deliberately a SEPARATE pub(crate) routine invoked
    /// explicitly from `library_store::from_json` before `normalize`, not folded
    /// into the pure `normalize`.
    ///
    /// Resolution key: `book.path().canonicalize().unwrap_or_else(|_| raw)` — the
    /// same rule `add_canonical` uses. A missing file fails `canonicalize`, so its
    /// key is its raw path: it can never be merged away and its path is preserved
    /// verbatim (a singleton group whose key equals its own raw path). Books are
    /// grouped by key in first-seen (vec) order; each group collapses to one
    /// survivor whose `path` is set to the canonical key (upgrading a now-resolvable
    /// raw path even for a group of one). Cross-member field merge is deterministic:
    /// `last_page` = max, `page_count` = first `Some` in vec order, `overrides` =
    /// first non-empty in vec order, `title` = the first member's title.
    /// `last_opened` is repointed from a merged-away / pre-upgrade spelling to the
    /// survivor's canonical key (a genuine orphan is left for `normalize` to clear).
    pub(crate) fn recanonicalize_and_merge(&mut self) {
        let books = std::mem::take(&mut self.books);
        // First-seen key order, the group members, and a remap from each book's
        // ORIGINAL stored path to its group's canonical key (for `last_opened`).
        let mut order: Vec<PathBuf> = Vec::new();
        let mut groups: HashMap<PathBuf, Vec<Book>> = HashMap::new();
        let mut remap: HashMap<PathBuf, PathBuf> = HashMap::new();
        for book in books {
            let key = book
                .path()
                .canonicalize()
                .unwrap_or_else(|_| book.path().to_path_buf());
            remap.insert(book.path().to_path_buf(), key.clone());
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
        // Repoint a `last_opened` spelled as a merged-away / pre-upgrade path to
        // its survivor's canonical key. A path absent from `remap` (a genuine
        // orphan) is left as-is for `normalize`'s orphan-clear.
        if let Some(last_opened) = self.last_opened.take() {
            self.last_opened = Some(remap.get(&last_opened).cloned().unwrap_or(last_opened));
        }
        self.books.sort_by(book_order);
    }

    /// Re-sort the shelf into natural title order (with canonical path tie-break)
    /// and enforce the `last_opened` invariant. Called on load
    /// (`library_store::load_from`) so libraries persisted before natural
    /// ordering converge to the canonical sort on the next save; it also repairs
    /// any otherwise-unsorted `books` vec and clears an orphan `last_opened`
    /// (a path that is no longer present in `books`).
    pub(crate) fn normalize(&mut self) {
        self.books.sort_by(book_order);
        // Clear a `last_opened` whose path is no longer in the shelf (e.g.
        // after an external edit removed a book without going through `remove`).
        if let Some(ref p) = self.last_opened {
            if !self.books.iter().any(|b| b.path() == p.as_path()) {
                self.last_opened = None;
            }
        }
    }

    /// Remove the book identified by `path`. Returns `false` when absent.
    /// Clears `last_opened` when it points at the removed book; preserves it
    /// when the removed book is different.
    pub fn remove(&mut self, path: &Path) -> bool {
        let before = self.books.len();
        self.books.retain(|b| b.path() != path);
        let removed = self.books.len() != before;
        if removed && self.last_opened.as_deref() == Some(path) {
            self.last_opened = None;
        }
        removed
    }

    /// Remove every book whose path is in `paths`, in ONE retain pass, and report
    /// the outcome. The natural-order invariant is preserved by construction:
    /// `retain` keeps the surviving books in their existing relative order, so the
    /// shelf stays sorted without a re-sort.
    ///
    /// Path identity uses the SAME comparison `remove` does (`Book::path() ==`),
    /// against the canonical paths stored at `add` time. Selection always
    /// originates from [`books()`](Library::books), so the inputs are already the
    /// stored canonical paths — no canonicalization happens here. The returned
    /// [`RemovalReport`] partitions the de-duplicated input into `removed` (matched
    /// a stored book) and `not_found` (matched none); a path duplicated in `paths`
    /// is counted once, and empty input yields an empty report. Clears
    /// `last_opened` when it points at one of the removed paths; preserves it
    /// otherwise.
    pub fn remove_many(&mut self, paths: &[PathBuf]) -> RemovalReport {
        // Collect the requested paths into a set: de-duplication falls out for
        // free (a repeated input is counted once in the report), and the retain
        // predicate below becomes an O(log M) membership test instead of an
        // O(M) linear scan per surviving book.
        let requested: BTreeSet<&Path> = paths.iter().map(PathBuf::as_path).collect();
        // Split into present (will be removed) and absent (reported not_found)
        // BEFORE the retain, while the books are still in the shelf to compare
        // against. Iterate the set in its sorted order for a deterministic report.
        let mut report = RemovalReport::default();
        for &path in &requested {
            if self.books.iter().any(|b| b.path() == path) {
                report.removed.push(path.to_path_buf());
            } else {
                report.not_found.push(path.to_path_buf());
            }
        }
        // ONE retain pass drops every requested book; survivors keep their relative
        // (already-sorted) order, so the natural-order invariant holds with no re-sort.
        self.books.retain(|b| !requested.contains(b.path()));
        // Clear `last_opened` when it points at one of the removed books.
        if self
            .last_opened
            .as_deref()
            .is_some_and(|p| requested.contains(p))
        {
            self.last_opened = None;
        }
        report
    }

    /// Re-insert previously removed `Book` entries and restore natural order.
    ///
    /// Unlike [`add`](Library::add), which derives a FRESH `Book` from a path
    /// (losing `last_page` / `page_count` / `overrides`), this re-inserts WHOLE
    /// `Book` values, so it can undo a [`remove_many`](Library::remove_many) with
    /// no data loss. It is the rollback primitive: a caller that removed a set of
    /// books, kept clones, and then failed to persist can hand the clones back to
    /// recover the exact pre-removal shelf (byte-identical re-serialization).
    ///
    /// De-duplicates by canonical path against the books already present (an entry
    /// whose path is already on the shelf is skipped, never duplicated), then
    /// re-sorts via the same `book_order` invariant `add` uses — the aggregate
    /// owns its ordering, callers never re-sort. Empty input is a no-op.
    pub fn restore(&mut self, books: Vec<Book>) {
        for book in books {
            if !self.books.iter().any(|b| b.path() == book.path()) {
                self.books.push(book);
            }
        }
        self.books.sort_by(book_order);
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

    /// Record the total `count` of pages for `path`. Returns `false` when the
    /// path is absent OR the value is unchanged (mirrors `set_last_page`, so
    /// callers can skip a save). `count` is a `NonZeroUsize`: the "must be
    /// positive" invariant (a measured count is never `0`, the unknown encoding)
    /// is now a type fact carried by the parameter, so no runtime guard is needed.
    pub fn set_page_count(&mut self, path: &Path, count: NonZeroUsize) -> bool {
        match self.books.iter_mut().find(|b| b.path() == path) {
            Some(book) if book.page_count != Some(count) => {
                book.page_count = Some(count);
                true
            }
            _ => false,
        }
    }

    /// Record the view overrides for `path`. Returns `false` when the path is
    /// absent OR the value is unchanged (mirrors `set_last_page`/`set_page_count`
    /// so callers can skip a save). The aggregate owns this mutation; `Book` has
    /// no setter.
    pub fn set_overrides(&mut self, path: &Path, overrides: ViewOverride) -> bool {
        match self.books.iter_mut().find(|b| b.path() == path) {
            Some(book) if book.overrides != overrides => {
                book.overrides = overrides;
                true
            }
            _ => false,
        }
    }

    /// The view overrides for `path`, or an all-None override when `path` is
    /// absent (so callers always get a value to `resolve`).
    pub fn overrides_for(&self, path: &Path) -> ViewOverride {
        self.books
            .iter()
            .find(|b| b.path() == path)
            .map(Book::overrides)
            .unwrap_or_default()
    }

    /// Register a freshly opened book and report how to resume it. Idempotent add
    /// by canonical path, then back-fill the page count when known. `page_count`
    /// is `Some(n)` for a measured total and `None` when the total is unknown
    /// (an empty/fully-skipped source); the `None` arm skips the back-fill and
    /// leaves the stored count at its unknown encoding. Returns the resume
    /// position (the book's `ReadingProgress`) and whether the stored count
    /// changed, so the caller can decide to rebuild the carousel.
    /// No persistence I/O; the only filesystem touch is the best-effort
    /// `canonicalize` inside `add_canonical`, which is idempotent when `canonical`
    /// is already canonical (the result is unchanged, though the syscall still
    /// runs; as it is when read from `open_file`). `canonical` is the
    /// canonicalized open key (the same key `last_page`/`set_page_count` use).
    pub fn register_opened(
        &mut self,
        canonical: &Path,
        page_count: Option<NonZeroUsize>,
    ) -> OpenRegistration {
        // `add_canonical` returns the shelf's identity for this book — the same
        // owned `PathBuf` whether the book was newly added or already present —
        // so the subsequent lookup can never miss. Capturing it as an owned value
        // also ends the borrow on `self` before the `&mut self` calls below.
        let (stored, _added) = self.add_canonical(canonical.to_path_buf());
        let count_changed = page_count.is_some_and(|c| self.set_page_count(&stored, c));
        // Resolve the resume position from the book actually stored under `stored`
        // (the shelf's canonical identity), so the `last_opened` invariant (it is a
        // member of `books`) holds by construction. The lookup is total: `stored`
        // is exactly the key `add_canonical` placed (or found) in `books`.
        let resume = self
            .books
            .iter()
            .find(|b| b.path() == stored)
            .map(Book::progress)
            .expect("registered book must be present");
        self.last_opened = Some(stored);
        OpenRegistration {
            resume,
            count_changed,
        }
    }
}

/// Derived availability: whether the book's path currently resolves on disk.
///
/// This performs filesystem I/O (`Path::exists`) and is deliberately a free
/// function rather than a `Library` method: the `Library` aggregate is a pure
/// in-memory ordered set and stays I/O-free. Availability is NOT stored - an
/// unavailable book is kept (its reading position is preserved); removal is an
/// explicit user action.
pub fn book_is_available(book: &Book) -> bool {
    book.path().exists()
}

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
    fn book_derives_title_from_dotted_existing_dir_name() {
        let dir = tempfile::tempdir().unwrap();
        let folder = dir.path().join("Series.1997");
        std::fs::create_dir(&folder).unwrap();
        let book = Book::from_path(folder);
        assert_eq!(book.title(), "Series.1997");
    }

    #[test]
    fn book_title_falls_back_to_path_string_when_no_stem() {
        // Pathological path with no file_stem (e.g. root) falls back to the lossy
        // path string so the title is never empty.
        let book = Book::from_path(PathBuf::from("/"));
        assert!(!book.title().is_empty());
    }

    // --- display_title tests (the title rule Book::from_path delegates to) ---

    #[test]
    fn display_title_uses_dir_name_for_real_folder() {
        // A real existing folder: the rule takes the directory name, not the stem
        // (which would strip a dotted suffix).
        let dir = tempfile::tempdir().unwrap();
        let folder = dir.path().join("Series.1997");
        std::fs::create_dir(&folder).unwrap();
        assert_eq!(display_title(&folder), "Series.1997");
    }

    #[test]
    fn display_title_uses_file_stem_for_archive_path() {
        // A file path (non-directory): the rule takes the file stem, dropping the
        // extension.
        assert_eq!(
            display_title(Path::new("/manga/Cool Title.cbz")),
            "Cool Title"
        );
    }

    #[test]
    fn display_title_keeps_leading_dot_name_as_stem() {
        // A dotfile like `.cbz` is NOT a hidden-extension case to `file_stem`: the
        // whole `.cbz` is the stem (non-empty), so the empty filter does not fire
        // and the title is the dotfile name verbatim — never the lossy fallback.
        assert_eq!(display_title(Path::new("/manga/.cbz")), ".cbz");
    }

    #[test]
    fn display_title_falls_back_to_lossy_path_when_no_file_component() {
        // A non-dir path whose `file_stem` is None (a trailing `..` component) hits
        // the empty/None fallback arm: the lossy full path string is used so the
        // title is never empty. This is the same fallback the empty filter feeds
        // into, exercised via the only route `std::path` actually produces.
        let title = display_title(Path::new("/manga/.."));
        assert!(!title.is_empty());
        assert_eq!(title, "/manga/..");
    }

    #[test]
    fn display_title_falls_back_to_lossy_path_for_root() {
        // A root-ish path has no file_name / file_stem at all, so the rule falls
        // back to the lossy full path string.
        let title = display_title(Path::new("/"));
        assert!(!title.is_empty());
        assert_eq!(title, "/");
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
        let first = PathBuf::from("/manga/a.cbz");
        let second = PathBuf::from("/manga/b.cbz");
        assert!(lib.add(first.clone()).is_some());
        assert!(lib.add(second.clone()).is_some());
        lib.register_opened(&second, NonZeroUsize::new(24));

        assert!(
            lib.clear(),
            "clearing a populated library changes persisted state"
        );
        assert!(lib.books().is_empty(), "all books are removed");
        assert_eq!(lib.last_opened(), None, "last_opened is cleared");
    }

    #[test]
    fn add_orders_books_by_natural_title() {
        // Non-existent paths exercise the best-effort canonicalize fallback
        // (canonicalize fails - path is used verbatim).
        let mut lib = Library::new();
        assert!(lib.add(PathBuf::from("/manga/vol 10.cbz")).is_some());
        assert!(lib.add(PathBuf::from("/manga/vol 1.cbz")).is_some());
        assert!(lib.add(PathBuf::from("/manga/vol 2.cbz")).is_some());
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
        let a = a.canonicalize().unwrap();
        let b = b.canonicalize().unwrap();

        let mut lib = Library::new();
        assert!(lib.add(b.clone()).is_some());
        assert!(lib.add(a.clone()).is_some());

        let paths: Vec<&Path> = lib.books().iter().map(|book| book.path()).collect();
        assert_eq!(paths, vec![a.as_path(), b.as_path()]);
    }

    #[test]
    fn add_dedups_by_path_and_returns_false() {
        let mut lib = Library::new();
        assert!(lib.add(PathBuf::from("/manga/a.cbz")).is_some());
        assert!(
            lib.add(PathBuf::from("/manga/a.cbz")).is_none(),
            "adding an existing path must be a no-op returning None"
        );
        assert_eq!(lib.books().len(), 1);
    }

    #[test]
    fn add_returns_stored_canonical_path_then_none_on_duplicate() {
        // A real temp file so canonicalize succeeds and the stored path is the
        // canonical one; the first add returns it, the second (duplicate) is None.
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("Book.cbz");
        std::fs::write(&file, []).unwrap();
        let canonical = file.canonicalize().unwrap();

        let mut lib = Library::new();
        assert_eq!(
            lib.add(file.clone()),
            Some(canonical.as_path()),
            "first add must return the stored canonical path"
        );
        assert!(
            lib.add(file).is_none(),
            "adding the same path again must return None (duplicate)"
        );
        assert_eq!(lib.books().len(), 1);
    }

    #[test]
    fn add_canonicalizes_existing_path_for_identity() {
        let dir = tempfile::tempdir().unwrap();
        let folder = dir.path().join("Series.1997");
        std::fs::create_dir(&folder).unwrap();

        let mut lib = Library::new();
        assert!(lib.add(folder.join(".")).is_some());
        assert_eq!(lib.books().len(), 1);
        assert_eq!(
            lib.books()[0].path(),
            folder.canonicalize().unwrap().as_path()
        );
        assert_eq!(lib.books()[0].title(), "Series.1997");
    }

    #[test]
    fn remove_existing_returns_true_and_drops_book() {
        let mut lib = Library::new();
        let path = PathBuf::from("/manga/a.cbz");
        assert!(lib.add(path.clone()).is_some());
        assert!(lib.remove(&path));
        assert!(lib.books().is_empty());
    }

    #[test]
    fn remove_absent_returns_false() {
        let mut lib = Library::new();
        assert!(!lib.remove(Path::new("/manga/missing.cbz")));
    }

    #[test]
    fn remove_many_removes_multiple_and_reports_them() {
        let mut lib = Library::new();
        for name in ["a.cbz", "b.cbz", "c.cbz"] {
            assert!(lib.add(PathBuf::from(format!("/manga/{name}"))).is_some());
        }
        let report =
            lib.remove_many(&[PathBuf::from("/manga/a.cbz"), PathBuf::from("/manga/c.cbz")]);
        assert_eq!(
            report.removed,
            vec![PathBuf::from("/manga/a.cbz"), PathBuf::from("/manga/c.cbz")]
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
            assert!(lib.add(PathBuf::from(format!("/manga/{name}"))).is_some());
        }
        // Remove the two middle volumes; vol 1 and vol 11 must remain in order.
        let report = lib.remove_many(&[
            PathBuf::from("/manga/vol 2.cbz"),
            PathBuf::from("/manga/vol 10.cbz"),
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
        assert!(lib.add(PathBuf::from("/manga/a.cbz")).is_some());
        let report = lib.remove_many(&[
            PathBuf::from("/manga/a.cbz"),
            PathBuf::from("/manga/missing.cbz"),
        ]);
        assert_eq!(report.removed, vec![PathBuf::from("/manga/a.cbz")]);
        assert_eq!(
            report.not_found,
            vec![PathBuf::from("/manga/missing.cbz")],
            "an input matching no book is reported, not removed"
        );
        assert!(lib.books().is_empty());
    }

    #[test]
    fn remove_many_empty_input_yields_empty_report() {
        let mut lib = Library::new();
        let book = PathBuf::from("/manga/a.cbz");
        assert!(lib.add(book.clone()).is_some());
        // Register it as last-opened so we can verify it is preserved.
        lib.register_opened(&book, None);
        let report = lib.remove_many(&[]);
        assert_eq!(report, RemovalReport::default());
        assert!(report.removed.is_empty());
        assert!(report.not_found.is_empty());
        assert_eq!(lib.books().len(), 1, "no input removes nothing");
        assert_eq!(
            lib.last_opened(),
            Some(book.as_path()),
            "empty remove_many must not disturb last_opened"
        );
    }

    #[test]
    fn remove_many_counts_duplicate_input_once() {
        let mut lib = Library::new();
        assert!(lib.add(PathBuf::from("/manga/a.cbz")).is_some());
        // The same path passed twice must be removed once and reported once.
        let report =
            lib.remove_many(&[PathBuf::from("/manga/a.cbz"), PathBuf::from("/manga/a.cbz")]);
        assert_eq!(
            report.removed,
            vec![PathBuf::from("/manga/a.cbz")],
            "a duplicated input path is counted once"
        );
        assert!(report.not_found.is_empty());
        assert!(lib.books().is_empty());
    }

    #[test]
    fn restore_reinserts_full_books_and_restores_natural_order() {
        // remove_many returns only paths; the rollback primitive must put back
        // WHOLE books (carrying last_page/page_count/overrides) and re-sort.
        let mut lib = Library::new();
        for name in ["vol 1.cbz", "vol 2.cbz", "vol 10.cbz"] {
            assert!(lib.add(PathBuf::from(format!("/manga/{name}"))).is_some());
        }
        // Give the middle volume a non-default reading position before removal.
        assert!(lib.set_last_page(Path::new("/manga/vol 2.cbz"), 7));
        assert!(lib.set_page_count(
            Path::new("/manga/vol 2.cbz"),
            NonZeroUsize::new(20).unwrap()
        ));

        // Clone the two outer volumes, remove them, then restore the clones.
        let removed: Vec<Book> = lib
            .books()
            .iter()
            .filter(|b| {
                b.path() == Path::new("/manga/vol 1.cbz")
                    || b.path() == Path::new("/manga/vol 10.cbz")
            })
            .cloned()
            .collect();
        let report = lib.remove_many(&[
            PathBuf::from("/manga/vol 1.cbz"),
            PathBuf::from("/manga/vol 10.cbz"),
        ]);
        assert_eq!(report.removed.len(), 2);
        let titles: Vec<&str> = lib.books().iter().map(|b| b.title()).collect();
        assert_eq!(titles, vec!["vol 2"], "only the unremoved book survives");

        lib.restore(removed);
        let titles: Vec<&str> = lib.books().iter().map(|b| b.title()).collect();
        assert_eq!(
            titles,
            vec!["vol 1", "vol 2", "vol 10"],
            "restore re-inserts and restores natural order"
        );
        // The untouched middle volume kept its position; the restored ones are intact.
        assert_eq!(lib.last_page(Path::new("/manga/vol 2.cbz")), 7);
    }

    #[test]
    fn restore_preserves_per_book_data_that_add_would_lose() {
        // The add()-trap: re-adding by path yields a FRESH book (last_page 0,
        // page_count None, overrides all-None).  restore must re-insert the WHOLE
        // clone, keeping last_page / page_count / overrides.
        let mut lib = Library::new();
        let p = PathBuf::from("/manga/a.cbz");
        assert!(lib.add(p.clone()).is_some());
        assert!(lib.set_last_page(&p, 42));
        assert!(lib.set_page_count(&p, NonZeroUsize::new(100).unwrap()));
        let ov = crate::view_override::ViewOverride {
            reading_direction: Some(crate::view_modes::ReadingDirection::Rtl),
            ..crate::view_override::ViewOverride::none()
        };
        assert!(lib.set_overrides(&p, ov));

        let clone: Vec<Book> = lib.books().to_vec();
        assert_eq!(lib.remove_many(std::slice::from_ref(&p)).removed.len(), 1);
        assert!(lib.books().is_empty());

        lib.restore(clone);
        assert_eq!(lib.books().len(), 1);
        assert_eq!(
            lib.last_page(&p),
            42,
            "restore keeps last_page (add would reset to 0)"
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
        let p = PathBuf::from("/manga/a.cbz");
        assert!(lib.add(p.clone()).is_some());
        let clone: Vec<Book> = lib.books().to_vec();
        // The book is still present; restoring its clone is a no-op (dedup by path).
        lib.restore(clone);
        assert_eq!(
            lib.books().len(),
            1,
            "restore must not duplicate an existing path"
        );
    }

    #[test]
    fn restore_empty_input_is_noop() {
        let mut lib = Library::new();
        assert!(lib.add(PathBuf::from("/manga/a.cbz")).is_some());
        lib.restore(Vec::new());
        assert_eq!(lib.books().len(), 1, "empty restore changes nothing");
    }

    #[test]
    fn restore_round_trips_to_byte_identical_json() {
        // The rollback contract: remove + restore must reproduce the EXACT
        // pre-removal serialization (the add()-trap would break this).
        // A non-default ViewOverride on one of the removed books makes the
        // comparison non-vacuous for the `overrides` field (a default/empty
        // override is omitted by `skip_serializing_if`, so the byte-identical
        // check would vacuously pass without this).
        let mut lib = Library::new();
        for name in ["a.cbz", "b.cbz", "c.cbz"] {
            assert!(lib.add(PathBuf::from(format!("/manga/{name}"))).is_some());
        }
        assert!(lib.set_last_page(Path::new("/manga/b.cbz"), 9));
        assert!(lib.set_page_count(Path::new("/manga/b.cbz"), NonZeroUsize::new(50).unwrap()));
        let ov = crate::view_override::ViewOverride {
            reading_direction: Some(crate::view_modes::ReadingDirection::Rtl),
            ..crate::view_override::ViewOverride::none()
        };
        assert!(lib.set_overrides(Path::new("/manga/b.cbz"), ov));
        let before = lib.to_json().unwrap();

        let removed: Vec<Book> = lib
            .books()
            .iter()
            .filter(|b| b.path() != Path::new("/manga/a.cbz"))
            .cloned()
            .collect();
        lib.remove_many(&[PathBuf::from("/manga/b.cbz"), PathBuf::from("/manga/c.cbz")]);
        lib.restore(removed);
        assert_eq!(
            lib.to_json().unwrap(),
            before,
            "remove + restore must be byte-identical"
        );
    }

    #[test]
    fn last_page_is_zero_for_unknown_path() {
        let lib = Library::new();
        assert_eq!(lib.last_page(Path::new("/manga/missing.cbz")), 0);
    }

    #[test]
    fn set_last_page_updates_and_round_trips() {
        let mut lib = Library::new();
        let path = PathBuf::from("/manga/a.cbz");
        assert!(lib.add(path.clone()).is_some());
        assert!(lib.set_last_page(&path, 42));
        assert_eq!(lib.last_page(&path), 42);
    }

    #[test]
    fn set_last_page_false_when_absent() {
        let mut lib = Library::new();
        assert!(!lib.set_last_page(Path::new("/manga/missing.cbz"), 12));
    }

    #[test]
    fn set_last_page_false_when_unchanged() {
        let mut lib = Library::new();
        let path = PathBuf::from("/manga/a.cbz");
        assert!(lib.add(path.clone()).is_some());
        assert!(lib.set_last_page(&path, 3));
        assert!(!lib.set_last_page(&path, 3));
    }

    #[test]
    fn page_count_opt_is_none_for_fresh_book() {
        let book = Book::from_path(PathBuf::from("/manga/a.cbz"));
        assert_eq!(book.page_count_opt(), None);
    }

    #[test]
    fn set_page_count_updates_and_returns_true() {
        let mut lib = Library::new();
        let path = PathBuf::from("/manga/a.cbz");
        assert!(lib.add(path.clone()).is_some());
        assert!(lib.set_page_count(&path, NonZeroUsize::new(42).unwrap()));
        assert_eq!(lib.books()[0].page_count_opt(), Some(42));
    }

    #[test]
    fn set_page_count_false_when_absent() {
        let mut lib = Library::new();
        assert!(!lib.set_page_count(
            Path::new("/manga/missing.cbz"),
            NonZeroUsize::new(12).unwrap()
        ));
    }

    #[test]
    fn set_page_count_false_when_unchanged() {
        let mut lib = Library::new();
        let path = PathBuf::from("/manga/a.cbz");
        assert!(lib.add(path.clone()).is_some());
        assert!(lib.set_page_count(&path, NonZeroUsize::new(3).unwrap()));
        assert!(!lib.set_page_count(&path, NonZeroUsize::new(3).unwrap()));
    }

    #[test]
    fn book_without_page_count_field_deserializes_to_unknown() {
        // Backward compatibility: an older `Book` JSON object that predates the
        // `page_count` field must still deserialize, with the missing field
        // defaulting to the unknown encoding (stored `0`) which surfaces as
        // `None` through `page_count_opt`.
        let value = serde_json::json!({
            "path": "/manga/a.cbz",
            "title": "a",
            "last_page": 5,
        });
        let book: Book = serde_json::from_value(value).unwrap();
        assert_eq!(book.path(), Path::new("/manga/a.cbz"));
        assert_eq!(book.last_page(), 5);
        assert_eq!(book.page_count_opt(), None);
    }

    #[test]
    fn progress_is_unread_for_freshly_added_book() {
        let mut lib = Library::new();
        let path = PathBuf::from("/manga/a.cbz");
        assert!(lib.add(path.clone()).is_some());
        let p = lib.books()[0].progress();
        assert!(p.is_unread(), "freshly added book must be unread");
        assert_eq!(p.reached(), 0);
        assert_eq!(p.total(), None);
    }

    #[test]
    fn progress_reflects_last_page_and_page_count() {
        let mut lib = Library::new();
        let path = PathBuf::from("/manga/a.cbz");
        assert!(lib.add(path.clone()).is_some());
        assert!(lib.set_last_page(&path, 3));
        assert!(lib.set_page_count(&path, NonZeroUsize::new(10).unwrap()));
        let p = lib.books()[0].progress();
        assert_eq!(p.reached(), 3);
        assert_eq!(p.total(), Some(10));
        assert_eq!(p.current(), 4);
        let expected: f32 = 3.0 / 10.0;
        assert!(
            (p.fraction() - expected).abs() < 1e-6,
            "fraction should be ~0.3, got {}",
            p.fraction()
        );
    }

    #[test]
    fn book_is_available_reflects_path_existence() {
        let dir = tempfile::tempdir().unwrap();
        let folder = dir.path().join("Series.1997");
        std::fs::create_dir(&folder).unwrap();
        let present = Book::from_path(folder.clone());
        let missing = Book::from_path(PathBuf::from("/manga/missing.cbz"));
        assert!(book_is_available(&present));
        assert!(!book_is_available(&missing));
    }

    #[test]
    fn register_opened_adds_fresh_book_and_back_fills_count() {
        let mut lib = Library::new();
        let path = PathBuf::from("/manga/a.cbz");
        let reg = lib.register_opened(&path, NonZeroUsize::new(10));
        assert_eq!(lib.books().len(), 1, "fresh open must add the book");
        assert!(reg.count_changed, "0 -> 10 must report a count change");
        assert!(reg.resume.is_unread(), "fresh book resumes as unread");
        assert_eq!(reg.resume.reached(), 0);
        assert_eq!(reg.resume.total(), Some(10));
    }

    #[test]
    fn register_opened_is_idempotent_and_reports_no_change_when_count_unchanged() {
        let mut lib = Library::new();
        let path = PathBuf::from("/manga/a.cbz");
        lib.register_opened(&path, NonZeroUsize::new(10));
        let reg = lib.register_opened(&path, NonZeroUsize::new(10));
        assert_eq!(lib.books().len(), 1, "re-opening must not duplicate");
        assert!(
            !reg.count_changed,
            "an unchanged count must report no change"
        );
    }

    #[test]
    fn register_opened_skips_back_fill_when_count_unknown() {
        // `None` means the total is unknown: the `is_some_and` guard skips
        // set_page_count entirely, so the stored count stays at its unknown
        // encoding and surfaces as `total() == None`.
        let mut lib = Library::new();
        let path = PathBuf::from("/manga/b.cbz");
        let reg = lib.register_opened(&path, None);
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
        let path = PathBuf::from("/manga/a.cbz");
        assert!(lib.add(path.clone()).is_some());
        assert!(lib.set_last_page(&path, 5));
        let reg = lib.register_opened(&path, NonZeroUsize::new(10));
        assert_eq!(reg.resume.reached(), 5, "resume reflects the recorded page");
        assert_eq!(reg.resume.current(), 6, "current is reached + 1");
    }

    #[test]
    fn register_opened_twice_preserves_recorded_last_page() {
        // Re-opening a book must NOT reset its resume position: the idempotent
        // add inside register_opened keeps the existing entry (and its last_page).
        let mut lib = Library::new();
        let p = PathBuf::from("/manga/a.cbz");
        lib.register_opened(&p, NonZeroUsize::new(10));
        assert!(lib.set_last_page(&p, 42));
        let reg = lib.register_opened(&p, NonZeroUsize::new(10));
        assert_eq!(lib.books().len(), 1, "re-open must not duplicate the book");
        assert_eq!(reg.resume.reached(), 42, "re-open must preserve last_page");
        assert!(!reg.count_changed, "same count on re-open is no change");
    }

    #[test]
    fn register_opened_reports_count_change_on_new_nonzero_count() {
        // A legitimately changed (non-zero -> different non-zero) page count is
        // reported as count_changed, flowing set_page_count's "did it move" branch.
        let mut lib = Library::new();
        let p = PathBuf::from("/manga/a.cbz");
        assert!(lib.register_opened(&p, NonZeroUsize::new(10)).count_changed);
        let reg = lib.register_opened(&p, NonZeroUsize::new(12));
        assert!(reg.count_changed, "10 -> 12 is a real change");
        assert_eq!(reg.resume.total(), Some(12));
    }

    #[test]
    fn register_opened_with_non_canonical_path_keeps_invariant() {
        // Invariant: after register_opened, last_opened is ALWAYS Some and points
        // at a stored book — even when the caller passes a non-canonical path that
        // diverges from the stored canonical key. We exercise this with a real file
        // reached via a `..` detour. On macOS, /tmp → /private/tmp also exercises
        // symlink divergence when tempdir lives there. Resolving the resume target
        // from the stored canonical identity (not the raw argument) is what makes
        // the lookup total; this previously degraded to None on divergence.
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("Book.cbz");
        std::fs::write(&file, []).unwrap();

        // Build a non-canonical path: enter a subdirectory and navigate back up.
        // If the tempdir has no subdirectory yet, create one purely for the detour.
        let sub = dir.path().join("sub");
        std::fs::create_dir(&sub).unwrap();
        let non_canonical = sub.join("..").join("Book.cbz");

        let mut lib = Library::new();
        lib.register_opened(&non_canonical, None);

        // last_opened is set, and its path is a book actually in the shelf.
        let lo = lib
            .last_opened()
            .expect("register_opened must always set last_opened")
            .to_path_buf();
        assert!(
            lib.books().iter().any(|b| b.path() == lo),
            "last_opened must point at a book actually in the shelf"
        );
        // On hosts where canonicalize resolves the `..`, the stored path equals
        // the canonical form.
        if let Ok(canonical) = non_canonical.canonicalize() {
            assert_eq!(
                lo, canonical,
                "last_opened must equal the canonicalized path stored by add"
            );
        }
    }

    #[test]
    fn new_book_has_empty_overrides() {
        let book = Book::from_path(PathBuf::from("/manga/a.cbz"));
        assert!(book.overrides().is_empty());
    }

    #[test]
    fn set_overrides_updates_and_reports_change() {
        let mut lib = Library::new();
        let p = PathBuf::from("/manga/a.cbz");
        assert!(lib.add(p.clone()).is_some());
        let ov = crate::view_override::ViewOverride {
            reading_direction: Some(crate::view_modes::ReadingDirection::Rtl),
            ..crate::view_override::ViewOverride::none()
        };
        // First set changes the value -> true.
        assert!(lib.set_overrides(&p, ov));
        assert_eq!(lib.overrides_for(&p), ov);
        // Setting the same value again is a no-op -> false (mirrors set_last_page).
        assert!(!lib.set_overrides(&p, ov));
    }

    #[test]
    fn set_overrides_clear_to_none_reports_change_and_empties() {
        let mut lib = Library::new();
        let p = PathBuf::from("/manga/a.cbz");
        assert!(lib.add(p.clone()).is_some());
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
        assert!(!lib.set_overrides(Path::new("/manga/missing.cbz"), ov));
    }

    #[test]
    fn overrides_for_absent_path_is_empty() {
        let lib = Library::new();
        assert!(lib
            .overrides_for(Path::new("/manga/missing.cbz"))
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
        let path = PathBuf::from("/manga/a.cbz");
        lib.register_opened(&path, None);
        assert_eq!(lib.last_opened(), Some(Path::new("/manga/a.cbz")));
    }

    #[test]
    fn register_opened_updates_last_opened_on_reopen() {
        // Opening a different book after the first one replaces last_opened.
        let mut lib = Library::new();
        let a = PathBuf::from("/manga/a.cbz");
        let b = PathBuf::from("/manga/b.cbz");
        lib.register_opened(&a, None);
        lib.register_opened(&b, None);
        assert_eq!(lib.last_opened(), Some(Path::new("/manga/b.cbz")));
    }

    #[test]
    fn register_opened_same_book_twice_preserves_last_opened() {
        // Re-opening the same book keeps last_opened pointing at it.
        let mut lib = Library::new();
        let path = PathBuf::from("/manga/a.cbz");
        lib.register_opened(&path, None);
        lib.register_opened(&path, None);
        assert_eq!(lib.last_opened(), Some(Path::new("/manga/a.cbz")));
    }

    #[test]
    fn remove_last_opened_book_clears_last_opened() {
        let mut lib = Library::new();
        let path = PathBuf::from("/manga/a.cbz");
        lib.register_opened(&path, None);
        assert!(lib.remove(&path));
        assert_eq!(lib.last_opened(), None);
    }

    #[test]
    fn remove_different_book_preserves_last_opened() {
        let mut lib = Library::new();
        let a = PathBuf::from("/manga/a.cbz");
        let b = PathBuf::from("/manga/b.cbz");
        lib.register_opened(&a, None);
        lib.add(b.clone());
        assert!(lib.remove(&b));
        assert_eq!(
            lib.last_opened(),
            Some(Path::new("/manga/a.cbz")),
            "removing a different book must not clear last_opened"
        );
    }

    #[test]
    fn remove_many_containing_last_opened_clears_it() {
        let mut lib = Library::new();
        let a = PathBuf::from("/manga/a.cbz");
        let b = PathBuf::from("/manga/b.cbz");
        lib.register_opened(&a, None);
        lib.add(b.clone());
        lib.remove_many(&[a.clone(), b.clone()]);
        assert_eq!(lib.last_opened(), None);
    }

    #[test]
    fn remove_many_not_containing_last_opened_preserves_it() {
        let mut lib = Library::new();
        let a = PathBuf::from("/manga/a.cbz");
        let b = PathBuf::from("/manga/b.cbz");
        lib.register_opened(&a, None);
        lib.add(b.clone());
        lib.remove_many(std::slice::from_ref(&b));
        assert_eq!(
            lib.last_opened(),
            Some(Path::new("/manga/a.cbz")),
            "removing an unrelated book must not clear last_opened"
        );
    }

    // --- bookmark tests (resume target = last_opened only when shelved) ---

    #[test]
    fn bookmark_is_some_when_last_opened_is_shelved() {
        let mut lib = Library::new();
        let path = PathBuf::from("/manga/a.cbz");
        lib.register_opened(&path, None);
        assert_eq!(
            lib.bookmark(),
            Some(Path::new("/manga/a.cbz")),
            "bookmark resolves to a shelved last_opened"
        );
    }

    #[test]
    fn bookmark_is_none_when_last_opened_is_none() {
        let mut lib = Library::new();
        // A shelved book but nothing opened yet: no resume target.
        assert!(lib.add(PathBuf::from("/manga/a.cbz")).is_some());
        assert_eq!(lib.last_opened(), None);
        assert_eq!(lib.bookmark(), None, "no last_opened means no bookmark");
    }

    #[test]
    fn bookmark_is_none_for_orphan_last_opened() {
        // last_opened points at a path NOT in books. We construct this via RAW
        // serde deserialization (NOT from_json, which runs normalize() and would
        // clear the orphan), so bookmark() must do the membership guard itself.
        let json = serde_json::json!({
            "books": [{"path": "/manga/a.cbz", "title": "a", "last_page": 0, "page_count": 0}],
            "last_opened": "/manga/gone.cbz"
        })
        .to_string();
        let lib: Library = serde_json::from_str(&json).unwrap();
        assert_eq!(
            lib.last_opened(),
            Some(Path::new("/manga/gone.cbz")),
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
            Some(Path::new("/manga/a.cbz")),
            "normalize must keep a valid last_opened"
        );
    }

    // --- recanonicalize_and_merge tests (re-canonicalize + duplicate merge on load) ---
    //
    // Each builds a `library.json` document with path spellings crafted directly
    // (bypassing add-time canonicalization), so two entries can name the SAME
    // on-disk file with different strings, then loads via `Library::from_json`.

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

        // Two entries naming the SAME file. Each field is set so the merge policy
        // is non-vacuous: max last_page comes from member[1]; the first `Some`
        // page_count is member[1]'s (member[0] is the unknown `0`); the first
        // non-empty override is member[0]'s; the title is member[0]'s.
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
            book.path(),
            canonical.as_path(),
            "survivor path is the canonical key"
        );
        assert_eq!(book.last_page(), 7, "last_page is the max across members");
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
            lib.books()[0].path(),
            canonical.as_path(),
            "a resolvable non-canonical path is upgraded to canonical on load"
        );
        assert_eq!(
            lib.books()[0].last_page(),
            5,
            "the book's data survives the upgrade"
        );
    }

    #[test]
    fn recanonicalize_preserves_missing_path_unchanged() {
        // A path that does not resolve (canonicalize fails) must be kept verbatim
        // — neither dropped nor merged. This is the regression guard for the
        // missing-file case.
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
            lib.books()[0].path(),
            Path::new(raw),
            "the raw path is preserved unchanged"
        );
        assert_eq!(lib.books()[0].last_page(), 9);
    }

    #[test]
    fn recanonicalize_repoints_last_opened_to_survivor() {
        // last_opened spelled as the non-canonical (merged-away) path must be
        // repointed to the surviving canonical path. Without the repoint,
        // normalize's orphan-clear would reset it to None.
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
            Some(canonical.as_path()),
            "last_opened is repointed to the surviving canonical path"
        );
    }

    #[test]
    fn recanonicalize_keeps_books_in_natural_order() {
        // After re-canonicalization/merge the shelf must still obey book_order.
        // Missing paths are kept verbatim, so this also confirms the sort runs
        // over the post-merge survivors.
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
