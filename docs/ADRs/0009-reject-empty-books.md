# ADR-0009: Validate book emptiness with a core probe at the boundaries

- Status: Accepted
- Decided: 2026-06-05
- Related: [ADR-0002](0002-layered-two-crate-architecture.md) (the core↔UI boundary governs where
  the rule may live), [ADR-0004](0004-archive-abstraction-and-extraction.md) (`PageSource` /
  `ArchiveLoader` — what "a page" is), [ADR-0006](0006-reading-position-value-object.md) (the
  `page_count == 0` "unknown" sentinel this rule constrains)
- Spec / brainstorm: `.claude/plans/reject-empty-books-design.md`

## Context

A folder or archive containing no images (`png`/`jpg`/`jpeg`) could be added to the library and
opened. It rendered as a blank cover with a "1 / 0" page counter. Such an item is not a book and
must not be loadable as one. The behavior had to be enforced at THREE moments — when a source is
added, when it is opened, and when its cover is loaded in the background — and a source that is
merely UNREADABLE (I/O error, unsupported format, corrupt archive) or temporarily MISSING (an
unmounted drive) must NOT be treated as empty: an unreadable source is a different failure, and a
missing-path book must keep its existing gray-out rather than being silently deleted.

The constraint is the layered architecture (ADR-0002): `gashuu-core` is headless and the collection
layer (`Library`) is a pure in-memory aggregate that performs no I/O. The emptiness check requires
opening the source — I/O — so where it lives is the design question.

## Decision

Define the domain rule "a valid book has >= 1 image page" ONCE in `gashuu-core`, expressed as a
type, and enforce it at the three UI boundaries that admit a source; the boundaries only "skip or
remove + notify".

1. **One core probe, returning a type.** `ArchiveLoader::probe_page_count(path) -> Result<NonZeroUsize,
   CoreError>` opens the source via the existing `ArchiveLoader::open`, counts `list_pages().len()`,
   and returns `Ok(NonZeroUsize)` for `1+` pages or `Err(CoreError::EmptyBook { path })` for `0`.
   The `NonZeroUsize` makes "a valid book with zero pages" unrepresentable (the same lift-into-the-type
   discipline as the `page_count` boundary, ADR-0006 / patterns.md). `CoreError::EmptyBook` is a new
   `#[non_exhaustive]` variant, so the addition is non-breaking.
2. **"Empty" and "unreadable" are strictly distinct.** `EmptyBook` is raised ONLY on a CLEAN open
   that counts zero pages. I/O errors and `UnsupportedFormat` propagate UNCHANGED — an unreadable
   source is never reclassified as empty. At the cover worker this is the pure
   `should_signal_empty(open_result, count) = open_result.is_ok() && count == 0`.
3. **Three UI hooks, no re-derivation of the rule.** Add (probe off the UI thread via
   `add_loader::probe_path` (now `add_controller::probe_path`), then `apply_outcomes` → `AddReport { added, skipped }`, issue 206):
   reject empty OR unreadable before insert, and persist the probed count on a genuine
   insert so a fresh add shows "1 / N" immediately. Open (`OpenBookUseCase::run` →
   `OpenOutcome::EmptyBookRejected { title, removed, save_error }`): on a clean zero-page open, bail out
   BEFORE the recents push / settings save / `register_opened`, remove the book if present, re-save,
   and stay on the Library. Cover-load (`marshal_empty_book` → the `empty-book-detected(string)` Slint
   callback): the worker signals; the UI-thread handler removes, purges the cover cache, rebuilds the
   carousel, and notifies.
4. **`Library::add` / `register_opened` are unchanged** — no I/O enters the collection layer; the
   probe runs in `ArchiveLoader` (the I/O dispatch layer) and the hooks live in the UI.
5. **A missing-path book is NOT removed.** Removal happens only when a scan SUCCEEDS and confirms
   zero images; the existing `is_available()` gray-out is preserved for an unmounted drive.

## Alternatives considered

- **(A) A core probe at the three boundaries (chosen).** The rule lives once, as a type, in the
  headless layer it belongs to; the UI hooks are thin "skip or remove + notify". The collection
  layer stays I/O-free.
- **(B) Validate inside `Library::add()`.** Rejected: it pushes I/O (opening the source) into the
  pure in-memory collection layer, violating the ADR-0002 boundary, and `register_opened` would
  double-scan the same source.
- **(C) UI-layer-only checks.** Rejected: it scatters the domain rule across `main.rs` and
  `app.rs`, duplicating the "count pages, reject zero" decision at three call sites with no single
  authoritative definition — the rule would drift between the sites on the next edit.

## Consequences

### Positive

- The "is this a book?" rule has exactly one definition (`probe_page_count`), unit-tested in core
  (folder / zip with images → `Ok(N)`; empty folder / image-free zip → `EmptyBook`; nonexistent →
  `Io`; text file → `UnsupportedFormat`).
- An empty source can no longer be added or opened; an existing entry that turns up empty is
  auto-removed. The "1 / 0" blank-cover state is gone.
- Fresh adds show their real `total` immediately (probed count persisted at add time), without
  waiting for the background cover prefetch.
- The core stays headless and `Library` stays I/O-free (boundary intact).

### Costs / trade-offs accepted

- Add-time probing is SYNCHRONOUS on the UI thread. Zip probing reads only the central directory and
  folder probing is a shallow `max_depth(1)` walk — both light — but a huge batch on a network drive
  could lag. Moving probing off-thread is deferred (YAGNI) until it proves slow in practice.
  (RESOLVED — see the Amendment.)
- The open-time and cover-time removal paths can race for the same book; idempotency rests on
  `Library::remove`'s bool rather than a lock (the race loser stays silent). Accepted as simpler than
  serializing the two paths.
- A folder whose images live only in SUBfolders probes to `EmptyBook` (the walk is `max_depth(1)`,
  consistent with the viewer) — by design, even though a user might expect recursion.

## Implementation notes (as-built deltas)

- **Side-effect ordering caught at recon.** The spec said "bail out before the recents push", but
  `OpenBookUseCase::run` pushed recents AND saved settings INSIDE the open-`Ok` arm, before
  `page_count` was known. The fix DEFERRED those side effects past the emptiness check so they fire
  only for a non-empty book. Recorded in [patterns.md](../patterns.md), "Insert a guard before X".
- **Worker → UI action via a root Slint callback.** The cover worker cannot capture the `!Send` `Rc`
  removal state, so it marshals a `Send` `PathBuf` and invokes `empty-book-detected(string)` under
  the epoch guard; the UI thread does the removal. Recorded in [patterns.md](../patterns.md), "Worker
  → UI ACTION via a Slint callback".
- **A pinned title replica.** `app.rs::derive_title` byte-for-byte replicates `Book::from_path`'s
  derivation (which is `pub(crate)` and unreachable from the UI crate), used only when an empty source
  was never added; four mirror-contract tests pin it. Recorded in [patterns.md](../patterns.md),
  "Replicate-and-PIN". (Superseded 2026-06-06 by DDD Wave 2 #150: Wave 1 #149 made the derivation
  public as `gashuu_core::display_title`; the replica and its mirror-contract tests are deleted.)
- **Open-path cover purge added (DDD Wave 2 #150, 2026-06-06).** The original ship deferred the
  cover purge to the cover-load path only; Wave 2 judged that asymmetry an implementation gap (a
  removed book's cached cover became unreachable garbage) and the open-time bail-out now purges too,
  via the shared `app::remove_empty_book` transaction.
- **No new dependencies; `LIBRARY_VERSION` unchanged.** `CoreError::EmptyBook` is the only new public
  surface in core; the three notices (`notice-added-books-skipped`, `notice-no-books-added-empty`,
  `notice-empty-book-removed`) are added to the Fluent catalog (en + ja) via `i18n/dynamic.rs`.
- **Related book-identity limitation (Wave-1 DDD refactor).** Decision 4 keeps `Library::add` I/O-free:
  its caller supplies the add-time `canonical_identity` snapshot as the de-dup key. A book added while
  its file was MISSING keeps a non-canonical identity, so a later re-add under the canonical form can
  create a DUPLICATE entry with separate `last_page` / `page_count` / `overrides`. This is a distinct
  concern from emptiness (identity, not page count) and is recorded as a known limitation in
  [patterns.md](../patterns.md), "book identity is the add-time canonical snapshot". The explicit
  load-time `recanonicalize_and_merge` self-heal resolves and merges such entries.

## Amendment 2026-07-25: the rejected open closes the viewer; probing is off-thread at BOTH boundaries

- **Decision 3's open hook gains a step.** After `remove_empty_book` and BEFORE returning
  `OpenOutcome::EmptyBookRejected`, `OpenBookUseCase::run` (in its apply half) calls
  `ViewerState::close()`. The post-rejection state is `open_file() == None`, `page_count() == 0`,
  no source mounted.
- **Why.** `open_path_with_policy` SUCCEEDS on an empty folder/archive, so the previous behavior
  left the zero-page source mounted with `open_file = Some(<the just-removed book>)`. That showed
  the removed book in the title bar; let the Library's Down key pass its only guard
  (`open_file().is_none()`) and enter a 0-page Viewer, violating this ADR's own "the Viewer never
  shows a 0-page book" consequence; silently closed whatever book WAS open; and made the `AppExit`
  route see a phantom open book and skip reconciling runtime view modes into global `Settings`.
- **The viewer is CLOSED, not restored to the previously open book.** The outgoing book's position
  and per-book override were already persisted by the leave-point write at the top of the apply
  half, so nothing is lost; this mirrors the bulk-remove use case's close-don't-restore precedent.
  `close()` needs no companion viewport reset — it zeroes the cache/source/count/index while
  deliberately preserving display modes and `cache_config`. `OpenBookUseCase::apply_probed`
  separately clears the `DialogSession` pending-inherit intent when it replaces the open source.
- **The Costs bullet is resolved.** Add probing moved off the UI thread with issue 206 (Decision 3
  has said so since; the Costs bullet has contradicted it ever since). The single-book OPEN path is
  now off-thread as well: the archive open, the full listing, canonicalization and the
  `is_dir`/`exists` stats run on a rayon worker via `open_controller::probe_open`, while ALL state
  mutation, both library saves and the optional settings save stay on the UI thread. The observable
  ordering is preserved — the `OpenDifferentBook` leave point still runs first in the apply
  half, and exactly one leave-point write happens per APPLIED open (a superseded probe writes
  nothing).
- Decisions 1, 2, 4 and 5 are unaffected; `probe_page_count` semantics and the missing-path rule are
  unchanged. See [architecture.md](../architecture.md), "open_controller" and "open_book".
