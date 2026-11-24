//! A naturally ordered, de-duplicated shelf keyed by opaque book IDs.
//!
//! Callers supply identity and display titles. On desktop, the load path
//! explicitly re-resolves legacy locations before normalizing the shelf.

use crate::reading_progress::ReadingProgress;
use crate::view_override::ViewOverride;
use crate::BookId;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeSet, HashMap};
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

/// Shelf ties deliberately compare the raw ID strings, which can differ from `BookId` path order.
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

/// Group books by key in first-seen order and collapse each group with `merge_group`.
/// Return unsorted survivors and a remap from each member's original ID to its group key.
fn merge_by_key(
    books: Vec<Book>,
    mut key_of: impl FnMut(&Book) -> BookId,
) -> (Vec<Book>, HashMap<BookId, BookId>) {
    let mut order = Vec::new();
    let mut groups: HashMap<BookId, Vec<Book>> = HashMap::new();
    let mut remap = HashMap::new();
    for book in books {
        let key = key_of(&book);
        remap.insert(book.id().clone(), key.clone());
        let members = groups.entry(key.clone()).or_default();
        if members.is_empty() {
            order.push(key);
        }
        members.push(book);
    }

    let survivors = order
        .into_iter()
        .map(|key| {
            let members = groups.remove(&key).expect("group for first-seen key");
            merge_group(key, members)
        })
        .collect();
    (survivors, remap)
}

/// An ordered shelf with at most one book per ID after load on every target.
/// Books are sorted by natural title order with the raw ID as the tie-break.
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
    /// canonical form keeps the raw path as its key, so the stored path is
    /// preserved verbatim and only exact duplicates share that key. Books are grouped
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
        let (books, remap) = merge_by_key(std::mem::take(&mut self.books), |book| {
            let canonical = canonical_identity(Path::new(book.id().as_str()));
            BookId::from_canonical_path(&canonical).unwrap_or_else(|_| book.id().clone())
        });
        self.books = books;
        // Repoint a `last_opened` spelled as a merged-away / pre-upgrade path to its
        // survivor's canonical key. A path absent from `remap` is left for normalize.
        if let Some(last_opened) = self.last_opened.take() {
            self.last_opened = Some(remap.get(&last_opened).cloned().unwrap_or(last_opened));
        }
        self.books.sort_by(book_order);
    }

    /// On load, merge exact duplicates to enforce one book per ID on every target,
    /// sort by natural title order with the raw ID as the tie-break, and clear
    /// `last_opened` if its ID is absent from the shelf.
    pub(crate) fn normalize(&mut self) {
        let (books, _) = merge_by_key(std::mem::take(&mut self.books), |book| book.id().clone());
        self.books = books;
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
#[path = "library/tests.rs"]
mod tests;
