# Architecture

This is the L2 reference document — the as-built MODULE MAP for the gashuu codebase. It was
migrated from the `## Architecture` section of `CLAUDE.md`. Architectural DECISIONS (the "why"
behind each choice) live in [docs/ADRs/](ADRs/README.md); this document is the current module
inventory: what exists, where it lives, and what it does.

---

## crates/gashuu-core (headless domain + I/O)

Slint-independent domain + I/O layer. See [ADR-0002](ADRs/0002-layered-two-crate-architecture.md)
for the rationale behind the two-crate split. Keep the layer boundary strict.

### PageSource trait

Requires `Send + Sync` so `Arc<dyn PageSource>` can be shared with rayon worker threads during
prefetch. `skipped_count(&self)->usize` is a TRAIT method as of PR6 — default `0`, overridden by
`FolderSource`/`ZipSource`/`RarSource` so the `Arc<dyn PageSource>` from `ArchiveLoader` exposes
it uniformly and `MockPageSource`/future sources need no change.

See [ADR-0004](ADRs/0004-archive-abstraction-and-extraction.md) for the `PageSource` abstraction
decision.

### FolderSource

Top-level directory walk with natural filename ordering (`max_depth(1)`, no recursion).

### ZipSource

PR6, `page_source/zip.rs`. A `PageSource` over a ZIP/CBZ archive using the SYNCHRONOUS `zip`
crate — flattens nested images (any image at any depth is a page, unlike `FolderSource`'s
`max_depth(1)`). Lock-free: each `read_bytes` opens its OWN `File` + `::zip::ZipArchive` so rayon
prefetch threads decompress fully in parallel with NO shared mutable state.

### RarSource

PR7, `page_source/rar.rs`. A `PageSource` over a RAR/CBR archive using the SYNCHRONOUS `unrar`
crate (bundles C++ UnRAR built by `cc`; extraction ONLY — no Rust RAR encoder exists), flattening
nested images like `ZipSource`. Lock-free via reopen + sequential-skip (RAR has NO random access):
each `read_bytes` opens its OWN `::unrar::Archive` + `open_for_processing()`, then
`read_header()`/`skip()` walks forward to the target's `seq_index` before `read()`.

### naming.rs

PR6. Shared image-extension recognition (`IMAGE_EXTS`, `has_image_ext`) and archive-entry path
validation (`MAX_ENTRY_BYTES`, `enclosed_name`) — all `pub(crate)`. `IMAGE_EXTS`/`has_image_ext`
were extracted from `folder.rs` so `FolderSource`/`ZipSource`/`RarSource` recognise images
identically. PR7 added `enclosed_name` (traversal/zip-slip guard rejecting absolute /
root-or-prefix / any `..` paths, mirroring `zip`'s protection for RAR entries) and moved
`MAX_ENTRY_BYTES` here from `zip.rs` (neutral shared 500 MB archive-entry ceiling imported by
both `zip.rs` and `rar.rs`). Filename ordering logic (`natural_cmp` and its helpers) was
extracted to `ordering.rs` in #82.

### ordering.rs

PR-82, `ordering.rs`. Shared numeric-aware natural-ordering comparator: `pub(crate) natural_cmp`
plus private helpers `take_digits`/`cmp_numeric`. Numeric-aware so `vol 1 < vol 2 < vol 10`
(digit runs are compared by numeric value, not lexicographically). Extracted from
`page_source::naming` (#82) so the comparator is reachable from both page-source filename sorting
(`FolderSource`/`ZipSource`/`RarSource`) and `Library` book ordering — a private submodule helper
in `naming.rs` could not be reused across the crate.

### ArchiveLoader

PR6, `archive_loader.rs`. `open(path) -> Arc<dyn PageSource>` dispatch — directory→`FolderSource`,
else a `Kind {Zip, Rar}` enum resolved by `ext_kind` (no I/O; `.cbz`/`.zip`→Zip,
`.cbr`/`.rar`→Rar, case-insensitive) preferred, else `magic_kind` sniff (`PK` ZIP
signatures→`ZipSource`; `Rar!\x1A\x07`→`RarSource`), else `UnsupportedFormat` (returns `Arc` not
`Box` to fit `set_source`).

### image_ops::decode

Returns raw RGBA8 + dimensions. Gained an explicit pixel-count guard `check_pixel_limit`/`MAX_PIXELS`
+ `CoreError::ImageTooLarge` in PR5, ahead of the `Limits`-bounded decode. PR8a extracted a PRIVATE
`decode_dynamic(&[u8]) -> Result<DynamicImage, CoreError>` holding the shared two-layer bomb guard —
header pre-read + `check_pixel_limit` + `Limits`-bounded decode — so BOTH `decode` and the new
`decode_thumbnail(&[u8], max_side) -> Result<DecodedImage, CoreError>` route through it and the bomb
guard lives in ONE place; a dedicated test proves `decode_thumbnail` inherits the early
`check_pixel_limit` rejection.

See [ADR-0003](ADRs/0003-image-loading-and-caching.md) for image loading decisions.

### thumbnail

PR8a, `thumbnail.rs`. `generate_thumbnails(source: Arc<dyn PageSource>, max_side, cancelled: Arc<AtomicBool>, on_ready: F)` —
SYNCHRONOUS, rayon `par_iter` over all pages invoking `on_ready(index, Result<DecodedImage, CoreError>)`
as each completes (arbitrary order), BLOCKING until done or `cancelled` flips (polled TWICE per
page: before read AND before callback); per-page failure is delivered as `Err` (never panics);
`DEFAULT_THUMB_MAX_SIDE`=160; headless (no slint/tracing), same "testable synchronous core; UI
owns the fire-and-forget spawn" philosophy as `ImageCache`. PR-V added the single-page sibling
`generate_cover(source: Arc<dyn PageSource>, max_side) -> Result<DecodedImage, CoreError>` (a
downscaled thumbnail of page index 0, the book's cover; `Err(IndexOutOfRange{index:0,len:0})` on a
0-page source, decode errors propagated); re-exported from the crate root and consumed by the UI's
`cover_loader.rs`.

### thumbnail_cache

PR-T, `thumbnail_cache.rs`. On-disk PNG cache for page/cover thumbnails under the OS cache dir
(`ProjectDirs("", "", "gashuu").cache_dir()/covers`); `with_dir(PathBuf)` is the tempfile-testable
seam. `put(key, &DecodedImage)` PNG-encodes the RGBA at exact dimensions and writes atomically
(temp-file-then-rename); `get(key) -> Option<DecodedImage>` reads `<dir>/<key>.png` and decodes,
returning `None` on any missing/unreadable/corrupt file (a cache miss, never panics). `cache_key(path,
mtime_secs, max_side)` derives a stable 16-hex-char filename via FNV-1a (NOT `DefaultHasher`; see
docs/patterns.md). Headless (no slint/tracing). The cover carousel consumes it via the UI's
`cover_loader.rs` (PR-V, shipped).

### cache::ImageCache

LRU of `Arc<DecodedImage>` up to `DEFAULT_CAPACITY`=50 + background ±`DEFAULT_PREFETCH_RADIUS`=3
prefetch in front of any `PageSource`.

See [ADR-0003](ADRs/0003-image-loading-and-caching.md) for the LRU/prefetch decision.

### CacheConfig

PR59, `cache_config.rs`. Immutable value object holding the LRU `capacity` (clamped to `>= 1` in
`new`) and the prefetch `radius` (`0` = prefetch disabled). It owns the capacity invariant in its
constructor, so an invalid cache size is unrepresentable downstream. `Settings` keeps the raw
`cache_size`/`preload_pages` integers (serde-flat, JSON unchanged) and exposes
`cache_config() -> CacheConfig`; `ImageCache::new` and `ViewerState` consume the `CacheConfig`,
which is why `ImageCache::new`'s `NonZeroUsize::new(config.capacity()).unwrap()` is provably safe.
Intentionally not `Deserialize` (that would bypass the clamp). See
[patterns.md](patterns.md) for the value-object pattern.

### spread

`spread.rs`. Pure, Slint/tracing-free, reading-direction-agnostic page-pairing
(`spread_at`/`next_leading`/`prev_leading`/`normalize_leading` over
`Spread {leading, trailing: Option<usize>}`); pairing functions take `SpreadLayout` (never
`SpreadMode`/`Auto` — `Auto` is unreachable at the type level in pairing). Also exports
`SpreadContext` — an immutable cohesion wrapper bundling `(total, layout, cover)` that
delegates to those free fns, so call sites read as intent rather than positional-arg plumbing.

### viewport

`viewport.rs`. Pure, Slint/tracing-free, STATELESS zoom/pan geometry — fit-scale (`fit_scale`),
pan clamping (`clamp_offset`/`centered_offset`), cursor-anchored zoom (`anchored_zoom`),
`clamp_zoom`, with `ZOOM_MIN`=1.0/`ZOOM_MAX`=8.0, table-tested like `spread`.

### Settings

`settings.rs`. Persistent user settings serialized to JSON in the OS config dir via
`directories::ProjectDirs`. `CoverMode` joined the settings vocabulary in PR4, `FitMode` in PR5:
`reading_direction`/`spread_mode`/`cover_mode`/`fit_mode`. PR4a added `SpreadMode::Auto` and the
resolved type `SpreadLayout {Single, Double}` with `SpreadMode::resolve(aspect: f32) -> SpreadLayout` —
`SpreadLayout` is NOT persisted. `seen_guide` (PR8b): a `bool` (default `false`,
`#[serde(default)]`) the UI flips to `true` + saves once the first-run guide is dismissed;
`SETTINGS_VERSION` stays 1 and the frozen snapshot gained `"seen_guide": false` — same
forward/backward-compat treatment as `cover_mode`/`fit_mode`.

**This is the first use of `serde` in core.** The headless boundary still holds (no
slint/tracing). I/O shape: `load_from`/`save_to` take explicit paths (tempfile-testable);
`load`/`save` are thin OS-path wrappers. Corrupt-file recovery (warn + fall back to defaults)
lives in the UI (`main.rs`); core only returns typed `CoreError`:

- PR4 added `Settings(#[from] serde_json::Error)` and `NoConfigDir`
- PR5 added `ImageTooLarge`
- PR6 added `Zip(#[from] ::zip::result::ZipError)`, `EntryTooLarge { name, max }`, `UnsupportedFormat { path }`
- PR7 added `Rar(#[from] ::unrar::error::UnrarError)` (Display prefix `"rar archive error: "`)

Errors are typed with `thiserror` (`CoreError`, `#[non_exhaustive]`).

See [ADR-0005](ADRs/0005-settings-persistence.md) for the settings persistence decision.

### reading_progress

PR-60 (`total` lifted to `Option<usize>` in #65), `reading_progress.rs`. Transient, immutable core
value object `ReadingProgress { reached, total: Option<usize> }` (`Copy`) that NAMES the one durable
fact — how far a reader got — and centralises its derivation in ONE place: `current()` (1-based,
`reached + 1` saturating, always ≥ 1), `fraction()` (`0.0..=1.0`; an unknown total `None` AND a
defensive `Some(0)` both → `0.0` — no NaN/inf; stale `reached` past `total` clamps to `1.0`),
`is_unread()` (`reached == 0`). It is the single home of the unknown/zero-total guard and
the 1-based offset that BOTH the carousel (`library_model::carousel_data` via `Book::progress()`)
and the open-time resume (`Library::register_opened`) consume. Derived / transient — never
serialised; `library.json` stores only the bare `last_page` + `page_count` fields on `Book`.
Headless (no slint/tracing).

### view_override

`view_override.rs`. Two paired view-preference value objects (headless: no slint/tracing). The
PERSISTED, partial form `ViewOverride { reading_direction, spread_mode, cover_mode, fit_mode }` —
each an `Option<Enum>` where `None` means INHERIT the global default (an active choice, NOT
"unknown"); `Copy`, immutable, `Default`/`none()` == all-`None`, `is_empty()` == all-`None` (the
serde `skip_serializing_if` predicate on `Book::overrides`). The TRANSIENT, total form
`ResolvedView` — every field concrete, never persisted, produced ONLY by
`ViewOverride::resolve(&Settings) -> ResolvedView` (the single definition of the per-field
`unwrap_or(global)` fallback). Re-exported from the crate root (`pub use view_override::{ResolvedView, ViewOverride}`).
Consumed by the UI's `ViewerState::apply_resolved_view` (+ `ViewportState::set_fit` for the
viewport-owned `fit_mode`). See [ADR-0007](ADRs/0007-per-book-view-overrides.md) for the design
decision and [patterns.md](patterns.md) ("Partial/total override pair") for the value-object flavor.

### Library aggregate

PR-60 (signature retyped in #65; book ordering added in #82; per-book view overrides added in
this feature), `library.rs`.
`Library::register_opened(canonical: &Path, page_count: Option<NonZeroUsize>) -> OpenRegistration
{ resume: ReadingProgress, count_changed: bool }` centralises the open-time domain rule that
previously lived in `main.rs`'s `app::OpenBookUseCase::run`: idempotent add by canonical path
(dedup); page-count back-fill applied only for `Some(_)` (an unknown total = `None` is skipped);
resume lookup via `Book::progress()`. The positivity that PR-60 enforced with a runtime guard is
now a type fact — `set_page_count(_, count: NonZeroUsize)` makes `0` unrepresentable at the write
boundary, so there is no `debug_assert` in core and no `page_count > 0` guard at the call site
(#65). The reader side maps stored counts through `Book::page_count_opt() -> Option<usize>`
(stored `0 → None`), the accessor that `progress()` and `carousel_data` consume. `main.rs` now
just calls `register_opened` and `jump_to(reg.resume.reached())`, converting at the boundary with
`NonZeroUsize::new(page_count)` (a zero-page open → `None` → back-fill skipped), keeping the
domain rule out of the presentation layer (aligns with the core↔UI boundary, ADR-0002).
`count_changed` tells the caller whether to rebuild the carousel. `Book::progress() ->
ReadingProgress` is the per-book accessor that `carousel_data` and `register_opened` both use;
`library.json` serde shape is unchanged (only `last_page` + `page_count` are persisted on each
`Book`, `page_count` still a bare `usize` with `0` for unknown).

`Library` is the single source of truth for book ordering: **natural title order** (numeric-aware,
via `ordering::natural_cmp`) with the canonical path as a stable tiebreak for identically titled
books. `add()` sorts the book list after inserting; `normalize()` re-sorts on load
(`library_store::load_from`) so libraries persisted before #82 (insertion order) converge to
natural title order on the next launch. The presentation layer (`carousel_data`,
`cover_requests`) inherits this order by iterating `books()` — it does NOT sort independently,
which keeps carousel row indices and cover-request indices aligned.

Each `Book` also carries a `ViewOverride overrides` field (the per-book view-preference overrides;
`#[serde(default, skip_serializing_if = "ViewOverride::is_empty")]`, so an all-`None` override emits
no key and `library.json` stays byte-compatible with files written before this feature — see
[conventions.md](conventions.md), "Add an optional nested config"). The aggregate owns the mutation:
`Library::set_overrides(path, ViewOverride) -> bool` (idempotent-changed bool, same `false`==no-op
convention as `set_last_page`/`set_page_count`) and `Library::overrides_for(path) -> ViewOverride`
(returns `ViewOverride::none()` for an unknown path, so the caller can `resolve` unconditionally).
There is NO `Book` setter — `Book::overrides()` is read-only.

**PR-5 (#129)** added the bulk-delete rollback primitive: `Library::restore(books: Vec<Book>)` —
re-inserts the supplied `Book` clones, de-duplicating by canonical path (an entry whose path is
already present is skipped), then re-sorts via `book_order`. This is the counterpart to
`remove_many`: a caller that captured clones before removal and then failed to persist can hand
them back to recover the exact pre-removal shelf (byte-identical re-serialization). Avoids the
`add()`-trap (which resets `last_page`/`page_count`) because the full clones carry all existing
field values. Consumed exclusively by `remove_books_with_rollback` in `app.rs`.

---

## crates/gashuu (Slint presentation layer)

See [ADR-0001](ADRs/0001-gui-framework-slint.md) for the Slint framework decision and
[ADR-0002](ADRs/0002-layered-two-crate-architecture.md) for the two-crate split rationale.

### ViewerState

Navigation backed by `ImageCache`; drives a two-page spread via
`current_spread() -> Option<Result<SpreadImages, CoreError>>`, with `apply` moving in spread units.
PR6 `open_path(path)` dispatches via `ArchiveLoader` + skipped warn + a `last_open_skipped()` getter,
and `open_folder` now delegates to it. PR8a added `jump_to(page) -> bool` — routes through
`SpreadContext::normalize` (via `spread_ctx()`) so `index` stays a valid spread leading, clamps
out-of-range, guards `page_count==0` to avoid underflow, and returns whether it moved, mirroring
`set_viewport_size`'s "did it change → caller refreshes" convention — and
`current_source() -> Option<Arc<dyn PageSource>>`
retaining the opened `Arc` because `ImageCache` does not expose its source; `index()`/`page_count()`
lost their `#[allow(dead_code)]`, now used by the thumbnail-strip wiring.

PR8b added IDEMPOTENT value setters `set_spread_mode(SpreadMode)`/`set_cover_mode(CoverMode)`/`set_reading_direction(ReadingDirection)`
(all `-> bool`, same value → `false` no-op, mirroring `jump_to`'s "moved? → caller refreshes"
convention) for the settings dialog — `set_spread_mode`/`set_cover_mode` call `renormalize_index`,
`set_reading_direction` does NOT (pairing is direction-agnostic) — plus
`set_cache_config(cache_size, preload_pages)` which updates the fields `set_source` reads on the
NEXT open (the dialog's cache/preload edits would otherwise only take effect after relaunch, since
the fields are seeded once at `from_settings` and `set_source` reads ViewerState's own fields, not
live `Settings`).

PR-R added `open_file() -> Option<&Path>`: the CANONICAL path of the most recently successful
`open_path` (set via `path.canonicalize().unwrap_or(verbatim)`, matching `Library::add`'s policy;
`None` until the first `Ok`, reset by `set_source`, unchanged by a failed `open_path`). `main.rs`
reads it to form the write-back tuple `(canonical_path, index())` for the `Library` at every leave
point — see the resume/write-back scope entry and `docs/patterns.md`.

This-feature added `apply_resolved_view(ResolvedView)` — sets `reading_direction`/`spread_mode`/`cover_mode`
from a resolved view (the `fit_mode` field is applied by the caller via `ViewportState::set_fit`),
called after the source is set when opening/returning to a book so its per-book override (resolved
against global `Settings`) takes effect. Runtime modes are intentionally NOT reset on open — they are
re-seeded by this call — which is why the open-time global reconcile is a clobber trap (see
[patterns.md](patterns.md), "Write-direction invariant audit").

PR-S added two pure scrubber-support helpers:

- `scrub_fraction_to_page(fraction, page_count, rtl)` — pure, total, RTL-aware mapping of a
  `0..1` knob fraction to a raw 0-based page index (clamped, non-finite-safe). As of #71 it is the
  SINGLE LIVE source of that mapping: `Scrubber.slint` passes the raw clamped fraction up via
  `preview(float)`/`commit(float)` and `main.rs`'s `on_scrub_preview`/`on_scrub_commit` call this
  helper to resolve the page (the former in-Slint `drag-page` rounding is gone, so it no longer
  carries `#[allow(dead_code)]`). See [patterns.md](patterns.md) for the one-authoritative-side rule.
- `preview_is_double(page)` — returns whether a previewed page would land on a 2-page spread
  (using the same layout resolution the body uses) WITHOUT advancing the index; used by the
  scrubber preview to choose 1 vs 2 popover thumbnails.

**PR-5 (#129)** added `close(&mut self)`: drops the cache and source, zeroes `page_count`/`index`,
and clears `open_file` — returning `ViewerState` to the no-book-open state. Display modes
(`reading_direction`/`spread_mode`/`cover_mode`) and `cache_config`/`viewport_aspect` are
deliberately preserved (closing a book is not a settings reset). Called by
`RemoveBooksUseCase::run` when the open book is among the deleted ones; `RemoveBooksUseCase::run`
itself also calls `ui.set_current_book_name("".into())` (`app.rs`~:481) — `main.rs` only sets
this name at boot and on a successful open.

### ViewportState

`viewport.rs`. UI-layer mutable zoom/pan/fit + viewport size; delegates ALL clamping to core pure
fns.

### keymap

Direction-aware key token → `KeyCommand` — page turns, D/R/C mode toggles, plus
direction-independent zoom/fit commands and PR8a's `ToggleThumbnails` on `"t"`,
direction-INDEPENDENT like the zoom/fit keys.

### navigation

PR-0b, `navigation.rs`. Top-level screen state machine: `Screen { Library, Viewer }` enum +
`NavState` (private `screen` field, intent-named `to_library`/`to_viewer` transitions, boots to
`Library`; same private-field+intent-method convention as `ViewportState`). Free fns
`screen_to_index`/`index_to_screen` map the enum to/from the Slint `ViewerWindow.screen` int
property — contract: Library = 0, Viewer = 1. `screen_to_index` is an exhaustive match (a new
variant is a compile error); `index_to_screen` clamps out-of-range to `Library`.

`main.rs` holds the single `NavState` in `Rc<RefCell<…>>`; `go_to_library`/`go_to_viewer` seam
functions are the single chokepoints that flip `NavState`, push `screen` to the UI, and restore
focus. The Up arrow (`KeyCommand::GoToLibrary`, direction-independent) and the carousel
`open`/`move`/`back` callbacks route through them.

### app

`app.rs`. `OpenBookUseCase` — the open-a-book application use case extracted from `main.rs` in
#67. Holds six shared collaborators as `Rc<RefCell<…>>` fields: `ViewerState`, `Settings`,
`ViewportState`, `Library`, `ThumbnailController` (`Rc`), and `CoverController` (`Rc`). There
is NO `NavState` field — `NavState` stays in `main.rs`. **PR-3 (#114)** replaced the former
`(Vec<String>, Option<String>)` return value with two new types and a revised single export:

- `run(&self, path, skipped_detail) -> OpenOutcome` — writes back the previous book's position;
  opens the source; reconciles + saves settings; registers the book in `Library` and jumps to the
  resume page; rebuilds the carousel model; launches thumbnails. On failure returns
  `OpenOutcome::Error(String)` (pre-captured `format!("{e}")`); on success returns
  `OpenOutcome::Success(NoticesContent)`. **`run` does NOT transition screens** and contains ZERO
  `crate::i18n` imports — all formatting is deferred to `main.rs::finalize_open`.
- `NoticesContent` — neutral data struct (skipped count, `SkippedDetail`, optional save-error
  strings) passed to `i18n::dynamic::format_notices` in `finalize_open`. No locale logic inside.
- `OpenOutcome` / `SkippedDetail` / `NoticesContent` are in scope for `main.rs` via
  `use crate::app::{NoticesContent, OpenOutcome, SkippedDetail}`.

`main.rs` constructs one instance and shares it (`Rc`) into the three open-handler closures:
`on_open_folder`, `on_open_archive`, and `on_carousel_open`. All three call
`finalize_open(&ui, &state, &viewport, &localizer, outcome)` after `run` returns; only
`on_carousel_open` also calls `go_to_viewer`. See [patterns.md](patterns.md),
"OpenOutcome pattern" and "finalize_open helper".

Lives in the UI crate because it coordinates Slint components; `gashuu-core` is untouched.

`run` writes back the OUTGOING book's per-book view override (`write_back_view_override`) right
beside the position write-back at its top, and — per the per-book-overrides feature — does NOT
reconcile runtime modes into the GLOBAL `Settings` on its open-time save (the runtime still holds
the outgoing book's per-book modes there; reconciling would clobber the global defaults — see
[patterns.md](patterns.md), "Write-direction invariant audit"). It then applies the just-opened
book's `ResolvedView` via `ViewerState::apply_resolved_view` (+ `ViewportState::set_fit`).

**PR-5 (#129)** added the parallel destructive use case to the same module:

- `RemoveBooksUseCase` — the "remove the selected books" use case. Holds four shared collaborators
  as `Rc<RefCell<…>>` fields: `ViewerState`, `Library`, `LibrarySearchState`, and
  `LibrarySelectionState`. `run(&self, ui) -> RemoveOutcome` executes the full destructive
  transaction in the non-negotiable order: snapshot selected paths → mutate+save with rollback
  (via `remove_books_with_rollback`) → best-effort cover purge (`ThumbnailCache::purge_for`,
  current mtime, `COVER_MAX_SIDE=512`, `tracing::warn`-only on miss) → clear the viewer via
  `ViewerState::close()` and blank the title bar when the open book was deleted → recompute the
  search projection → clear the selection (success only; `SaveFailed` preserves it so the user can
  retry). Returns `RemoveOutcome::NoSelection` (empty selection guard), `RemoveOutcome::SaveFailed
  { error }` (shelf rolled back byte-identically via `Library::restore`; selection preserved), or
  `RemoveOutcome::Removed { n, closed_open_book }`. The caller (`main.rs`) rebuilds the carousel,
  clamps the focused index, and composes the status line from the outcome.
- `remove_books_with_rollback(library, paths, save)` — the transaction primitive: captures full
  `Book` clones before removal, calls `Library::remove_many`, then calls `save`; on `Err` calls
  `Library::restore(removed_books)` (byte-identical rollback — avoids the `add()`-trap that would
  reset `last_page`/`page_count`) and propagates the error. The injected `save` closure makes it
  unit-testable against a failing or succeeding save.
- `confirm_delete_content(loader, selection, search, library, open_file) -> ConfirmDeleteContent`
  — pure (no I/O, no Slint) builder for the confirm dialog body: title with total count, up to 10
  book titles in selection (BTreeSet path) order, an "…and M more" line when count > 10, an "N
  selected outside the current search" line when filtered-out books are included, and a warning
  when the open book is selected. The struct (`title`, `body_lines`, `info`, `warning`) holds
  fully localized strings — each field is resolved via `i18n::dynamic` inside the builder and is
  display-ready.

### per-book view-override seam fns (`main.rs`)

`pub(crate)` seam fns in `main.rs` (peers of `reconcile_settings`/`write_back_position`):
`write_back_view_override(&state, &viewport, &library)` snapshots the open book's four runtime
modes into its `ViewOverride` and saves the library — called at EVERY leave point (nav-away,
open-another, exit, Viewer settings-dialog close); a no-op when no book is open.
`apply_global_view_to_runtime(&settings, &state, &viewport)` mirrors the GLOBAL `Settings` view
modes into the runtime so the Library-screen settings dialog seeds from global (the inverse of
`reconcile_settings`). The shared `SettingsDialog` routes by `ui.get_screen()`: `0` (Library) →
`reconcile_settings` into global; `1` (Viewer) → `write_back_view_override` into the current book.
The exit-path `reconcile_settings` is gated on `open_file().is_none()`. See
[patterns.md](patterns.md), "Per-book view overrides: write-back-at-leave-point + screen-scoped
dialog routing".

### library_model

PR-C, `library_model.rs`. PURE (Slint-free) `Library` → carousel display-row mapping: a plain
`CarouselData` struct (`title`/`current`/`total`/`progress`/`available`) + `carousel_data(&Library)
-> Vec<CarouselData>` in natural title order (inherited from `Library::books()` — no independent
sort here; see `Library aggregate` in the core section). The carousel counterpart of `thumbnail_strip`'s row mapping —
keeps the derivation table-testable without a display backend. Each row is built from
`Book::progress()`: 1-based `current = ReadingProgress::current()` (`reached + 1`, saturating);
`progress = ReadingProgress::fraction()` (guarded so an unknown/zero total → `0.0`, overshoot clamps
to `1.0`); `total = ReadingProgress::total()` (now `Option<usize>`, #65). The free derivation no longer lives in `library_model`
— it is centralised in `ReadingProgress` (see core entry below). `available` via
`Library::is_available`. `carousel.rs`'s `to_carousel_item` adapter builds the `!Send`
`slint::Image` (a `slint::Image::default()` placeholder, filled in asynchronously by `CoverController`)
on the UI thread; `build_carousel_model` is the build+bind
chokepoint returning the `Rc<VecModel>` so PR-V/PR-L mutate the same model. `total` comes from
the persisted `Book::page_count` (PR-La): `0` until the count is known
(`progress` guarded to `0.0`), the real saved count afterwards — known either by opening the book or,
for a never-opened book, by `CoverController`'s background page-count prefetch (see `cover_loader.rs`),
which streams the count into this `total` and persists it. The `total: clamp_to_i32(total)`
saturating cast is unchanged.

### LibrarySearchState

#88, `library_model.rs` (pub(crate) struct). Owns the active search `query` and a set of
`forced_visible_paths` — books added in the current session that stay visible even when the
query would exclude them, until the next query change. Maintains `visible_indices: Vec<usize>` —
the filtered projection of library row indices in natural `Library::books()` order. Recomputes
after every mutation: `set_query(query, &Library)` (clears forced paths, then recomputes);
`force_visible(paths, &Library)` (dedups against the existing forced set, then recomputes).
`recompute(&Library)` is the entry point for LIBRARY-changed-only cases where neither the query
nor the forced set moved (startup seed, open-time backfill). `visible_indices()` is read-only and
always consistent with the last mutation. Pure helpers `book_matches` / `matching_indices` live
alongside it; `matching_indices` is the fast-path delegate when no paths are forced visible.

### LibrarySelectionState

Bulk-delete PR-2/PR-4 (#126/#128), `library_model.rs` (pub(crate) struct). Owns the active
selection as a `BTreeSet<PathBuf>` keyed by canonical path — the same key `Library::add` uses —
so the selection survives query changes and carousel rebuilds. Key mutators: `toggle(path)` (adds
or removes one path); `select_visible(search, library)` / `deselect_visible(search, library)`
(select/deselect only the visible projection — out-of-view selected paths are never touched,
preserving the "selection is orthogonal to the query" invariant; used by the Select-all button and
Cmd/Ctrl+A); `clear()` (success-path reset, called by `RemoveBooksUseCase` on success only). Key
accessors: `count()` (total selection size, across the whole library); `selected() -> impl
Iterator<Item=&Path>` (BTreeSet path-sorted order — deterministic for the confirm-dialog title
list and the `RemoveBooksUseCase` removal snapshot); `contains(path) -> bool`;
`all_visible_selected(search, library) -> bool` (drives the Select-all toggle direction).
`visible_selected_count(search, library) -> usize` counts how many selected paths appear in the
current visible projection (used by the toolbar's "N outside search" indicator and by
`confirm_delete_content`). Used as a collaborator by `RemoveBooksUseCase` (PR-5 #129).

### carousel

PR-58, `carousel.rs`. UI-thread adapter layer between `library_model` and Slint: `to_carousel_item` (private) maps a `CarouselData` row to a `CarouselItem` (placeholder `slint::Image::default()` cover); `build_carousel_model(library: &Library, indices: &[usize])` (pub(crate)) is a HEADLESS builder — no `ViewerWindow` arg — returning the `Rc<VecModel<CarouselItem>>`; a separate `bind_carousel_model(ui, model)` performs the Slint bind (the build/bind split enables unit tests over visible-index order); `cover_requests(library: &Library, indices: &[usize])` (pub(crate)) derives the per-book `CoverRequest` list, re-basing each request's `row` to the enumerated position in the filtered `indices` slice (not the library index), so cover targets stay aligned with the filtered carousel model; `thumb_state_at` (pub(crate)) re-fetches a row's thumbnail state for the scrubber preview.

### enum_adapters

PR-58, `enum_adapters.rs`. The 10 `pub(crate)` enum↔index adapters (the first 8 were previously inline in `main.rs`): `reading_direction_to_index`/`index_to_reading_direction`, `spread_mode_to_index`/`index_to_spread_mode`, `cover_mode_to_index`/`index_to_cover_mode`, `fit_mode_to_index`/`index_to_fit_mode`, and `language_to_index`/`index_to_language` (i18n PR). Each `index_to_*` clamps out-of-range to the first variant, mirroring the `index_to_screen` clamp policy in `navigation.rs`.

### i18n

Fluent i18n PR-1 (#112), `i18n/` module (`mod.rs` + `loader.rs` + `dynamic.rs`). **`messages.rs`
was deleted in PR-3 (#114)** — all runtime-composed strings now live in `i18n/dynamic.rs` (see
below). `Localizer` wraps an `i18n_embed::fluent::FluentLanguageLoader`; its `new(lang)`/`switch(lang)`
call `load_languages` then re-apply `set_use_isolating(false)`, and `panic!` on a load failure
(compile-time-embedded assets ⇒ programmer error — see [patterns.md](patterns.md), "Fluent loader").
All mutating methods take `&self` (the loader has interior mutability), so `main.rs` shares one
`Rc<Localizer>` into the Slint callbacks without a `RefCell`. `pub(crate) fn loader()` getter was
added in PR-3 (justified by `dynamic.rs` as the real consumer; per the dead-code-getter rule in
[patterns.md](patterns.md), this was deferred until the getter had a non-test caller). `loader.rs`
holds the `#[derive(RustEmbed)] struct Localizations` (embeds `i18n/`) and the exhaustive
`langid_for(Language) -> LanguageIdentifier` (no wildcard — the compile-time gate replacing the
former `messages.rs` exhaustive match). The completeness/parity/byte-oracle integration tests live
in `mod.rs`'s `#[cfg(test)]`. The `Language` enum stays in headless `gashuu-core`; ALL Fluent
machinery is UI-crate-only, per [ADR-0002](ADRs/0002-layered-two-crate-architecture.md). PR-2 (#113)
added `Localizer::apply(&self, ui: &ViewerWindow)` to `mod.rs`: the single chokepoint that resolves
every Fluent-served static string via `fl!()` and pushes it into the `Strings` global (next entry);
`main.rs` calls it at boot and after each `switch()` — see [patterns.md](patterns.md), "The
`Strings`-global push".

**`i18n/dynamic.rs`** (Fluent i18n PR-3, #114, NEW): Fluent-backed dynamic message functions that
replaced the deleted `messages.rs`. Each function takes a `&FluentLanguageLoader` borrowed from
`Localizer::loader()` and returns a freshly formatted `String`. Two aggregators drive the presentation
layer: `format_status(loader, &StatusContent) -> String` and `format_notices(loader, &NoticesContent)
-> Vec<String>` — they take the language-free content structs from `viewer_state.rs` and `app.rs`
respectively and apply the locale (see [patterns.md](patterns.md), "Neutral content structs" and
"OpenOutcome pattern"). `open_error` (`&dyn Display` form) is `#[cfg(test)]`-only; production always
goes through `open_error_str` with the pre-captured `OpenOutcome::Error(String)` payload.

**`i18n/` assets + `i18n.toml`** (crate root): one Fluent catalog per locale,
`i18n/{en,ja}/gashuu.ftl`, carrying the FULL vocabulary (both the former gettext msgids and the
former `msg_*` messages); `i18n.toml` declares `fallback_language = "en"` and `assets_dir = "i18n"`.
Message-ID naming convention: [conventions.md](conventions.md), "Fluent catalog message IDs".

### page_jump

`page_jump.rs`. PURE (Slint-free, table-tested) parser for the `ViewerPill` page-jump field:
`parse_page_jump(input: &str, total: usize) -> Option<usize>` maps a 1-based string input to a
0-based page index. Returns `None` for empty / all-whitespace / non-numeric input and for
`total == 0`; otherwise clamps the numeric value to `[1, total]` (treating `0` as `1`) and
subtracts 1. Replaces the removed `page_counter.rs`. The viewer wires the parsed index through
`ViewerState::jump_to` (same "did it move → caller refreshes" convention as the scrubber).

### Slint UI files

**`ui/components/`** (#71, NEW): shared single-purpose UI atoms/molecules, one `export`ed component
per file — `ProgressBar` (accent/`success` reading-progress fill), `PrimaryButton` (the accent CTA),
`ThumbnailCell` (the loaded/loading/failed/highlighted
cell shared by the page strip, the scrubber preview popover, and the library covers), `ViewerPill`
(#this-PR, NEW: the viewer glass-pill — floating page-jump field + thumbnail toggle + settings;
replaces the docked TitleBar), `NavBar`
(#83, NEW: the top-centered glass-pill Library nav — translucent fill + hairline border + top inner
highlight + drop shadow; Slint has no backdrop-blur so the glass effect is paint-only), `NavItem`
(#83, NEW: one circular icon capsule inside `NavBar`; hover/press glow via `Theme.accent-glow`;
non-focusable `TouchArea` with `accessible-role`/`accessible-label`/`accessible-action-default` for
screen-reader support), `Dropdown` (i18n PR, NEW: the Apple-HIG pull-down button used by the
language setting — the repo's first `PopupWindow`; the menu is lowered to the window root so the
dialog's clipping Flickable can't cut it off — see docs/patterns.md), `Segmented` (issue 102,
PR-A: token-driven horizontal segmented control replacing the std-widgets `ComboBox`; equal-width
cells with an accent-pill selected state, keyboard Left/Right navigation), `Stepper` (issue 102,
PR-A: token-driven integer stepper replacing `SpinBox`; surface-sunken capsule with accent ± glyphs,
keyboard Up/Down), `Toggle` (issue 102, PR-A: token-driven on/off switch replacing `CheckBox`; pill
track accent-on / `track-prog`-off, spring-animated knob), `SettingRow` (issue 103, PR-B: L1
alignment molecule — fixed label column + `@children` control slot spanning to the shared right
rail; `trailing` flag pushes compact atoms onto the rail via a leading stretch spacer),
`SelectionBadge` (bulk-delete PR-2: pure visual atom — accent disc with a centered white check mark,
overlaid on selected covers in selection mode; both Carousel `for` passes include it, rendered only
when `selected` is true), `SelectionToolbar` (bulk-delete PR-4/PR-5: selection-mode organism — count
pill / select-all capsule / exit capsule / **Delete (N)… `DangerButton`** (if-gated, hidden at
N=0; the only red element in the app's chrome; PR-5 #129); glass-pill matching `NavBar`'s
four-layer idiom, content-hugged width, overlaid below `NavBar` inside `Carousel` via an
`if-gate`; all human-visible text passed from Rust via `in` properties — deliberately
`Strings`-agnostic), and `DangerButton`
(bulk-delete PR-1: destructive-action atom — structural clone of `PrimaryButton` swapping accent for
`Theme.danger` red + `Theme.danger-glow` hover/focus ring; accessibility role/label/action-default
added here, `PrimaryButton` backport pending; first consumed by the PR-3 `ConfirmDialog`, and now
also the PR-5 `SelectionToolbar` delete button). Each references `Theme.*` via `../Theme.slint`;
consumers import via `import { X } from "components/X.slint"`. `build.rs` is unchanged — it compiles
the single entry `ui/ViewerWindow.slint` and import statements cascade. See
[docs/conventions.md](conventions.md) for the component RULES.

**`ui/assets/`** (#83, NEW): the repo's image assets — `file.svg`, `folder.svg` (#83), `plus.svg`
(the macOS combined "Add books" capsule glyph), `carousel.svg`
(the thumbnail-toggle icon in ViewerPill; replaced the original `slider.svg` glyph),
`chevron-down.svg` (i18n PR, NEW: the `Dropdown` chevron; 96px intrinsic size per the HiDPI
rasterization rule), and `check.svg` (bulk-delete PR-2, NEW: the single-path check mark recolored
via `colorize` to `Theme.text` white inside `SelectionBadge`), each a single-path SVG recolored at
runtime via Slint's `Image.colorize` property. Components reference
them with `@image-url(...)` paths relative to the consuming `.slint` file. `build.rs` is unchanged
(assets are reached transitively through the entry-file import cascade).

**`ui/Strings.slint`** (Fluent i18n PR-2, #113, NEW): `export global Strings` — the Fluent-served
static-string surface, 67 `in property <string>` slots (property name == Fluent message ID) with
English-literal defaults. Written exclusively from Rust by `Localizer::apply()`; `.slint` bindings
read `Strings.<prop>` in place of the removed `@tr()` calls. `ViewerWindow.slint` re-exports it
(`import` + `export { Strings }`) so Slint generates the `ui.global::<Strings>()` accessor. See
[docs/patterns.md](patterns.md), "The `Strings`-global push".

**`Carousel.slint`** (PR-0b shell; PR-C rendering; PR-L toolbar/CTA; #71 componentized; #83 glass-pill nav): Library
cover-flow carousel.
PR-0b froze the public contract (`CarouselItem` struct + `Carousel` component with `items`,
`focused-index`, callbacks `open(int)`/`move(int)`/`back()`, `public function focus-self()`); PR-C
filled in the rendering against that UNCHANGED contract: centered focused cover (accent ring) +
scaled/dimmed neighbors, a per-cover `ProgressBar` (#71 promoted the former file-private bar to the
shared `components/ProgressBar.slint`, reused for the focused-meta bar), a centered focused-book meta
block, a grayed broken-cover placeholder for
unavailable books, and the 0-book empty-state CTA. #71 also routes its covers and empty-state CTA
through the shared `ThumbnailCell`/`PrimaryButton` components. Covers start as placeholders
(`slint::Image::default()`); PR-V's `cover_loader.rs` streams the real cover images into the same
model row-by-row. PR-L added the add callbacks (today `add-books()`/`add-folder()`; `add-books` was
named `add-files` until the macOS combined picker landed) and wired the empty-state CTA
to `add-books()` (each restores focus via `focus-self()` after firing); PR-L's original left-aligned
two-`Button` text toolbar was REPLACED in #83 with a centered, icon-only glass-pill `NavBar` (two
`file`/`folder` `NavItem` capsules on Windows/Linux; on macOS a single combined `plus` capsule,
gated by the Rust-pushed `combined-add-picker` flag). The `NavBar` is declared as a SEPARATE LAST layer in the
component tree — paint order equals declaration order in Slint, so it renders on top of the cover-flow
without a z-index — and kept OUTSIDE the `FocusScope` so keyboard navigation remains carousel-owned;
the nav is mouse + screen-reader oriented. The `add-books()`/`add-folder()` callbacks and the
`focus-self()`-after-fire behavior are unchanged; `NavBar` simply forwards into them. The cover-flow
is rendered by a file-private `CoverCard` sub-component instantiated by TWO `for` passes over the model:
pass 1 is the always-on BACKING layer (`show: true` for every book), pass 2 (declared after) paints ONLY
the focused card so it draws ON TOP of its backing twin — Slint 1.x cannot set per-`Repeater`-item z, so
draw order is the only lever. Both passes bind identical geometry (each book keeps a persistent instance
in each pass), so the Left/Right slide still animates continuously with a seamless layer hand-off. The
enclosing row's `width`/`row-cy` are passed into `CoverCard` as `in` properties because a component ROOT
cannot read `parent`.

**`Theme.slint`** (PR-0b, NEW; #83 extended): single `global Theme` of visual tokens (colors, spacing,
radii, font sizes); components reference `Theme.<token>` instead of inline hex literals. #83 added the
glass-surface colors (`glass-fill`, `glass-border`, `glass-highlight`) and the golden-ratio nav sizing
tokens (`nav-icon`, `nav-capsule`, `nav-pill-height`, `nav-item-gap`, `nav-pill-pad`). Authoritative
values live in DESIGN.md; this file is the as-built note only.

**`ThumbnailStrip.slint`** (PR8a; #71): horizontal `Flickable` + `HorizontalLayout` + `for` over a
`VecModel` — the FIRST `VecModel`/`Repeater` use in the codebase since `ListView` is
vertical-only — over `struct ThumbnailItem { image, page, loaded, failed }`. #71 replaced the inline
per-cell markup with the shared `ThumbnailCell` component (border ring / background / radius /
loaded-loading-failed-highlighted states now live there).

**`thumbnail_strip.rs`** (`ThumbnailController`, PR-B / issue #30): owns the strip's
`Rc<VecModel<ThumbnailItem>>`, the epoch counter, and the cancel flag; `new(&ui)` builds and binds the
model via `ui.set_thumbnails`; `start(&self, ui_weak, source, page_count)` cancels any in-flight
generation, resets the model to `page_count` placeholders, and spawns the background worker. `main.rs`
constructs the controller once and calls `thumbs.start(...)` in every open handler, with no thumbnail
bookkeeping inline.

**`cover_loader.rs`** (`CoverController`, PR-V): a structural TWIN of `ThumbnailController` for the
Library carousel. Streams each book's real cover into the shared `VecModel<CarouselItem>`: derive
`cache_key(path, mtime, max_side)`, try `ThumbnailCache::get` (hit → set the row's `cover` on the UI
thread now), miss → `rayon::spawn` a worker that opens via `ArchiveLoader`, calls core `generate_cover`,
`ThumbnailCache::put`, then `invoke_from_event_loop` to set the row — under the SAME epoch + cancel
double-guard and Send/!Send discipline as the thumbnail strip (see [patterns.md](patterns.md)). The
cancel-rotation lives in a shared-shape private `rotate_cancel(&self) -> Arc<AtomicBool>` helper kept
IDENTICAL in both controllers. Covers render at `COVER_MAX_SIDE = 512` px — DECOUPLED from the strip's
`DEFAULT_THUMB_MAX_SIDE = 160` (a focused cover slot is far larger than a strip cell, so 160 px upscaled
blurry); `max_side` is in the cache key, so raising it auto-regenerates stale 160 px covers. The
controller ALSO prefetches each unopened book's REAL page count (fixing the "1 / 0" display): a request
flagged `needs_count` (`Book::page_count_opt() == None`) resolves the count in the background — the
cover-MISS worker reuses its archive open (`source.list_pages().len()` before `generate_cover`), a
cover-cache HIT spawns a count-only open (`spawn_count_only`). `marshal_total` streams the count to the
row's `total` for immediate display, and the `(path, count)` is queued in `pending_counts` for UI-thread
persistence — drained via `Library::set_page_count` + `save` at the next `start` and at shutdown
(`flush_counts`), so the count survives a relaunch (a worker can't touch the `!Send`
`Rc<RefCell<Library>>`, hence the queue). Callers build the `CoverRequest` list BEFORE `start` so the
`library.borrow()` is released before `start`'s persistence `borrow_mut`.

**`SettingsDialog.slint`** (PR8b, NEW; issue 102 replaced its std-widgets `ComboBox`/`SpinBox`/`CheckBox` with the token-driven `Segmented`/`Stepper`/`Toggle`/`Dropdown` atoms): modal overlay editing active settings; two-way `current-index <=> in-out-prop` +
`selected`/`edited`/`toggled` callbacks. Since issue 103/104 it is a **content-hug glass panel** (φ relocated into the component proportions; spec 2026-06-04) built from custom `components/` atoms; its footer "Shortcuts" link opens `ShortcutsOverlay`, and an `in property <int> focus-epoch` (bumped by `ViewerWindow.focus-settings()`) lets the parent re-focus this still-mounted dialog after the overlay closes.

**`ShortcutsOverlay.slint`** (issue 104, PR-C, NEW): a second glass modal listing the keyboard shortcuts read-only, stacked OVER the still-mounted `SettingsDialog` (both `show-settings` and `show-shortcuts` true). Reuses the settings glass recipe (`settings-w`/`settings-radius` + all glass tokens; only `shortcuts-h` is new). Its ancestor `FocusScope` traps every key so focus can't leak to the dialog underneath; closing returns focus to the dialog via the epoch seam. **PR-5 (#129)** extended the keyboard-shortcuts reference text to document the full selection grammar: `x`, `Space`, `Cmd/Ctrl+A`, `Delete`/`Backspace`, `Esc`.

**`components/ConfirmDialog.slint`** (issue 127, PR-3, NEW; wired in `ViewerWindow` for the bulk-delete path by PR-5 #129): a GENERIC two-choice confirm/cancel modal — no domain vocabulary. Every string on screen arrives through `in` properties (`title`, `body-lines: [string]`, `info-text`, `warning-text`, `confirm-label`, `cancel-label`) so the same component is reusable across confirm decisions. Clones the `SettingsDialog` / `ShortcutsOverlay` glass idiom (scrim + four-layer fake-glass object). Mounted in `ViewerWindow` behind `if root.show-confirm-delete` (an `if`-gate so the node is constructed only when needed). Cancel / Esc / backdrop click fire the `cancel` callback (Slint-side: set `show-confirm-delete = false`, restore carousel focus — selection PRESERVED); the confirm `DangerButton` fires the `confirm()` callback (`ConfirmDialog.slint:117`), which `ViewerWindow` forwards as `confirm => { root.confirm-delete-accepted(); }` (ViewerWindow.slint:591), and Rust registers on `ui.on_confirm_delete_accepted` to run `RemoveBooksUseCase` and dismiss the modal. `Enter` is wired to Cancel (the destructive action is never on `Enter`).

**`FirstRunGuide.slint`** (PR8b, NEW): dismissable once-only overlay; a local `GuideLine`
component dedupes the key-reference rows.

**`Theme.slint`** (PR-S, NEW; completed #70): a single Slint `global Theme` that centralises all visual design
tokens — colors, corner radii, spacing, font sizes, component sizes, shadow colors, motion durations, and font weights —
sourced from `/DESIGN.md`. ALL UI components reference `Theme.*`; the three previously-inline-hex dialogs (ThumbnailStrip, SettingsDialog, FirstRunGuide) were migrated in #70, and `scripts/check-tokens.sh` is now unconditionally blocking for the whole UI (no allowlist).

**`PageView.slint`**: the page canvas; hosts pan/zoom via a single `TouchArea`. Predates PR-S.
PR-S added a `reveal()` callback, fired on `changed mouse-x` / `changed mouse-y` (pointer-move),
which triggers the auto-hiding viewer chrome.

**`Scrubber.slint`** (PR-S; #71): bottom auto-hiding page-scrubber with a drag-time thumbnail
preview popover. Public surface: `in` properties `current-page` / `total-pages` / `rtl` /
`double` / `preview-a` / `preview-b` / `chrome-shown`; callbacks `preview(float)` / `commit(float)`
(#71 retyped these from `int`: the scrubber now passes the RAW clamped knob fraction and Rust owns
the fraction→page rounding — see [patterns.md](patterns.md)). Drag fires `preview` only;
pointer-release fires `commit`. Its preview thumbs use the shared `ThumbnailCell` component.

**`ViewerWindow.slint`**: extended in PR8b with the two `if root.show-X : Component` overlays
(last children = front), a "Settings…" toolbar button, the in/in-out properties + setter
callbacks, and a FocusScope key-guard. `main.rs` gained the dialog/guide wiring + 8 enum↔index
helper fns (since extracted to `enum_adapters.rs`) + `KEY_BINDINGS_HELP`. Extended in PR-0b with a two-screen model: `in property <int>
screen` gates the Library `Carousel` (screen 0) vs the Viewer body (screen 1) via
`visible: root.screen == N` (not `if` — see [patterns.md](patterns.md) for the Slint id-scoping
reason); Settings/Guide overlays remain viewer-scoped. Extended again in PR-S to mount the
`Scrubber` as auto-hiding chrome inside the screen-1 viewer,
driven by a `chrome-shown` bool + an idle `Timer`; chrome is revealed on pointer-move (via
`PageView.reveal()`), arrow-key presses, and scrubber drag. #71 mounted the shared `TitleBar`
component (bound to a new `current-book-name` in-prop — derived in `main.rs` from the post-open
`ViewerState::open_file()`, see [patterns.md](patterns.md)), and set a `min-width`/`min-height` floor on the window.

### rfd file/folder picker

PR6 `on_open_archive` → `rfd` `pick_file` filtered to cbz/zip. PR7 extended the filter to
cbz/zip/cbr/rar — the ONLY UI change in PR7 since `open_path` already dispatched via
`ArchiveLoader`. "Open Archive" button lives in `ViewerWindow.slint`.

PR-L added Library-side pickers: `on_add_books` (né `on_add_files`; filtered cbz/zip/cbr/rar —
`pick_files_or_folders` on macOS, where one NSOpenPanel picks archives AND folders in a single
panel, `pick_files` elsewhere; the platform split lives in `#[cfg]` blocks inside the one handler,
and the matching `combined-add-picker` bool pushed at boot collapses the NavBar's two add capsules
into one on macOS) and
`on_add_folder` (`pick_folder`, folder-as-one-book; its capsule is hidden on macOS). `main.rs` owns the library-add seam — `add_paths`
(dedup-aware insert, returns the count of NEW books), `build_carousel_model` (Library → `ModelRc<CarouselItem>`,
0-based `last_page` → 1-based `current`, real `total`/`progress` from persisted `Book::page_count` (PR-La),
placeholder cover), and the shared
`add_books_and_refresh` handler (insert → save → rebuild carousel → status line → restore carousel
focus; short-circuits when nothing new was added). The persisted `Library` lives in `main.rs` as
`Rc<RefCell<Library>>`, loaded at startup and seeded into `carousel-items` on boot.

### RGBA conversion

Converts core RGBA to `slint::Image::from_rgba8`.

### Logging

Logs via `tracing`; user-facing errors are formatted with `color-eyre` and shown in the status
bar.

---

## Why the two-crate split

Core stays headless and unit-testable (no display server); the UI is the only place that touches
Slint, pixel buffers, and logging.

See [ADR-0002](ADRs/0002-layered-two-crate-architecture.md) for the full rationale.
