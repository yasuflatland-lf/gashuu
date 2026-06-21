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
prefetch. `skipped_count(&self)->usize` is a TRAIT method — default `0`, overridden by
`FolderSource`/`ZipSource`/`RarSource` so the `Arc<dyn PageSource>` from `ArchiveLoader` exposes
it uniformly and `MockPageSource`/future sources need no change.

See [ADR-0004](ADRs/0004-archive-abstraction-and-extraction.md) for the `PageSource` abstraction
decision. See [ADR-0011](ADRs/0011-decoder-subprocess-isolation.md) for the planned subprocess
isolation of RAR/CBR and AVIF decoders.

### FolderSource

Top-level directory walk with natural filename ordering (`max_depth(1)`, no recursion).

### ZipSource

`page_source/zip.rs`. A `PageSource` over a ZIP/CBZ archive using the SYNCHRONOUS `zip`
crate — flattens nested images (any image at any depth is a page, unlike `FolderSource`'s
`max_depth(1)`). Lock-free: each `read_bytes` opens its OWN `File` + `::zip::ZipArchive` so rayon
prefetch threads decompress fully in parallel with NO shared mutable state.

### RarSource

`page_source/rar.rs`. A `PageSource` over a RAR/CBR archive using the SYNCHRONOUS `unrar`
crate (bundles C++ UnRAR built by `cc`; extraction ONLY — no Rust RAR encoder exists), flattening
nested images like `ZipSource`. Lock-free via reopen + sequential-skip (RAR has NO random access):
each `read_bytes` opens its OWN `::unrar::Archive` + `open_for_processing()`, then
`read_header()`/`skip()` walks forward to the target's `seq_index` before `read()`.

### naming.rs

Shared image-extension recognition (`IMAGE_EXTS`, `has_image_ext`) and archive-entry path
validation (`MAX_ENTRY_BYTES`, `enclosed_name`) — all `pub(crate)`. `IMAGE_EXTS`/`has_image_ext`
were extracted from `folder.rs` so `FolderSource`/`ZipSource`/`RarSource` recognise images
identically. `enclosed_name` is the traversal/zip-slip guard rejecting absolute /
root-or-prefix / any `..` paths, mirroring `zip`'s protection for RAR entries; `MAX_ENTRY_BYTES`
lives here (neutral shared 500 MB archive-entry ceiling imported by
both `zip.rs` and `rar.rs`). Filename ordering logic (`natural_cmp` and its helpers) was
extracted to `ordering.rs` in #82.

Two shared archive-entry helpers also live here so the rule is single-owned rather than
open-coded (and drifting) in each source. `classify_entry(name, is_dir, declared_size, max) ->
EntryClass {Page, Skip, Ignore}` is the page-membership decision both `ZipSource` and `RarSource`
make after `enclosed_name`: directories / non-images / macOS metadata are `Ignore`d as expected
noise, an oversized image is a counted `Skip`, everything else is a `Page` (the membership check
precedes the size check). `cap_or_reject(src, name, max, capacity_hint)` is the read-time streaming
size cap (`take(max + 1)` + length check → `EntryTooLarge`) shared by `FolderSource` and
`ZipSource`; `RarSource` cannot stream-cap (`unrar` materializes the whole entry) so it keeps its
declared-`unpacked_size` re-validation. The format-specific iteration (zip `by_index` vs rar
sequential `read_header`) and the zip-slip guard stay in each source.

### ordering.rs

`ordering.rs`. Shared numeric-aware natural-ordering comparator: `pub(crate) natural_cmp`
plus private helpers `take_digits`/`cmp_numeric`. Numeric-aware so `vol 1 < vol 2 < vol 10`
(digit runs are compared by numeric value, not lexicographically). Extracted from
`page_source::naming` (#82) so the comparator is reachable from both page-source filename sorting
(`FolderSource`/`ZipSource`/`RarSource`) and `Library` book ordering — a private submodule helper
in `naming.rs` could not be reused across the crate.

### ArchiveLoader

`archive_loader.rs`. `open(path) -> Arc<dyn PageSource>` dispatch — directory→`FolderSource`,
else a `Kind {Zip, Rar}` enum resolved by `ext_kind` (no I/O; `.cbz`/`.zip`→Zip,
`.cbr`/`.rar`→Rar, case-insensitive) preferred, else `magic_kind` sniff (`PK` ZIP
signatures→`ZipSource`; `Rar!\x1A\x07`→`RarSource`), else `UnsupportedFormat` (returns `Arc` not
`Box` to fit `set_source`). The reject-empty-books feature added the associated fn
`probe_page_count(path) -> Result<NonZeroUsize, CoreError>`: it `open`s the source, counts
`list_pages().len()`, and returns `Err(CoreError::EmptyBook { path })` on a clean open with zero
pages — the SINGLE home of the domain rule "a valid book has >= 1 image page". I/O /
`UnsupportedFormat` errors propagate unchanged ("empty" and "unreadable" are strictly distinct).
See [ADR-0009](ADRs/0009-reject-empty-books.md) and [patterns.md](patterns.md) ("Lift a domain rule
into ONE core type at the boundary").

### image_ops::decode

Returns raw RGBA8 + dimensions. Carries an explicit pixel-count guard `check_pixel_limit`/`MAX_PIXELS`
+ `CoreError::ImageTooLarge` ahead of the `Limits`-bounded decode. A PRIVATE
`decode_dynamic(&[u8]) -> Result<DynamicImage, CoreError>` holds the shared two-layer bomb guard —
header pre-read + `check_pixel_limit` + `Limits`-bounded decode — so BOTH `decode` and
`decode_thumbnail(&[u8], max_side) -> Result<DecodedImage, CoreError>` route through it and the bomb
guard lives in ONE place; a dedicated test proves `decode_thumbnail` inherits the early
`check_pixel_limit` rejection.

See [ADR-0003](ADRs/0003-image-loading-and-caching.md) for image loading decisions.

### thumbnail

`thumbnail.rs`. `generate_thumbnails(source: Arc<dyn PageSource>, max_side, cancelled: Arc<AtomicBool>, on_ready: F)` —
SYNCHRONOUS, rayon `par_iter` over all pages invoking `on_ready(index, Result<DecodedImage, CoreError>)`
as each completes (arbitrary order), BLOCKING until done or `cancelled` flips (polled TWICE per
page: before read AND before callback); per-page failure is delivered as `Err` (never panics);
`DEFAULT_THUMB_MAX_SIDE`=160; headless (no slint/tracing), same "testable synchronous core; UI
owns the fire-and-forget spawn" philosophy as `ImageCache`. The single-page sibling
`generate_cover(source: Arc<dyn PageSource>, max_side) -> Result<DecodedImage, CoreError>` (a
downscaled thumbnail of page index 0, the book's cover; `Err(IndexOutOfRange{index:0,len:0})` on a
0-page source, decode errors propagated); re-exported from the crate root and consumed by the UI's
`cover_loader.rs`.

### thumbnail_cache

`thumbnail_cache.rs`. On-disk PNG cache for page/cover thumbnails under the OS cache dir
(`ProjectDirs("", "", "gashuu").cache_dir()/covers`); `with_dir(PathBuf)` is the tempfile-testable
seam. `put(key, &DecodedImage)` PNG-encodes the RGBA at exact dimensions and writes atomically
(temp-file-then-rename); `get(key) -> Option<DecodedImage>` reads `<dir>/<key>.png` and decodes,
returning `None` on any missing/unreadable/corrupt file (a cache miss, never panics). `cache_key(path,
mtime_secs, max_side)` derives a stable 16-hex-char filename via FNV-1a (NOT `DefaultHasher`; see
docs/patterns.md). Headless (no slint/tracing). The cover carousel consumes it via the UI's
`cover_loader.rs`.

Issue 143 adds the GC half: `prune(max_bytes) -> PruneReport` sweeps the directory down to
`max_bytes` of `*.png` payload in ascending `(mtime, file name)` order, and reclaims stale
`.{key}.tmp` crash leftovers (older than an hour) regardless of the cap. `get` refreshes a hit's
mtime (touch-on-get, only after a successful decode), which makes the eviction order near-LRU —
key-orphaned covers (source mtime drifted past `purge_for`) are never read again, age to the
front, and disappear once the cap bites. Best-effort throughout (missing dir → zero report,
failed unlink skipped, foreign files never touched); the caller logs the returned `PruneReport`
(core stays log-free). The cap POLICY is the app layer's: `cover_loader.rs` owns
`COVER_CACHE_MAX_BYTES` (256 MiB) and `spawn_cache_prune()`, dispatched once at startup on a
rayon worker right after the initial cover stream.

### cache::ImageCache

LRU of `Arc<DecodedImage>` up to `DEFAULT_CAPACITY`=50 + background ±`DEFAULT_PREFETCH_RADIUS`=3
prefetch in front of any `PageSource`. Two access paths serve different callers: `get(index)`
decodes synchronously on a miss (kept for tests); `get_cached(index)` is a non-blocking pure
cache-hit probe that returns `None` on a miss without ever reading or decoding. `dispatch_handle()`
returns a cloneable `Send` `CacheDispatch` handle (`decode_and_cache`) that shares the live LRU via
`Arc<Inner>` — the rayon worker decodes on a miss, inserts into the shared cache, then spawns
neighbour prefetch. A racing prefetch or a concurrent worker that populated the same slot is detected
by the double-checked `cached()` call in `decode_and_cache`, so the last writer wins and a duplicate
decode is safely discarded.

See [ADR-0003](ADRs/0003-image-loading-and-caching.md) for the LRU/prefetch decision.

### CacheConfig

`cache_config.rs`. Immutable value object holding the LRU `capacity` (clamped to `>= 1` in
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
`directories::ProjectDirs`. The view-mode vocabulary it persists
(`reading_direction`/`spread_mode`/`cover_mode`/`fit_mode`/`language`/`key_bindings`) now lives in
`view_modes.rs` (see below); `Settings` is just one consumer. `seen_guide`: a `bool` (default `false`,
`#[serde(default)]`) the UI flips to `true` + saves once the first-run guide is dismissed;
`SETTINGS_VERSION` stays 1 and the frozen snapshot carries `"seen_guide": false` — same
forward/backward-compat treatment as `cover_mode`/`fit_mode`.

**This is the first use of `serde` in core.** The headless boundary still holds (no
slint/tracing). I/O shape: `load_from`/`save_to` take explicit paths (tempfile-testable);
`load`/`save` are thin OS-path wrappers. Corrupt-file recovery (warn + fall back to defaults)
lives in the UI (`main.rs`); core only returns typed `CoreError`:

- `Settings(#[from] serde_json::Error)` and `NoConfigDir`
- `ImageTooLarge`
- `Zip(#[from] ::zip::result::ZipError)`, `EntryTooLarge { name, max }`, `UnsupportedFormat { path }`
- `Rar(#[from] ::unrar::error::UnrarError)` (Display prefix `"rar archive error: "`)
- `EmptyBook { path }` (Display `"no images found in {path}"`) — raised by `ArchiveLoader::probe_page_count` on a clean open with zero image pages, distinct from the unreadable-source errors above (the reject-empty-books feature)

Errors are typed with `thiserror` (`CoreError`, `#[non_exhaustive]`).

See [ADR-0005](ADRs/0005-settings-persistence.md) for the settings persistence decision.

### view-mode vocabulary (`view_modes.rs`)

`view_modes.rs` (extracted from `settings.rs`). The ubiquitous-language enums for how pages are
displayed: `ReadingDirection`, `SpreadMode` + the resolved `SpreadLayout {Single, Double}` (with
`SpreadMode::resolve(aspect: f32) -> SpreadLayout`, which the UI layer calls so the pure `spread::*`
functions never see `Auto`; `SpreadLayout` is NOT persisted), `CoverMode`, `FitMode`, `Language`,
and the `KeyBindings` value object. This is the actual vocabulary; `Settings` (in `settings.rs`) is
one consumer, and the pure modules `spread`/`viewport`/`view_override` import it from here instead of
through the serde-persistence aggregate. Headless (serde only, no slint/tracing). External public
paths (`gashuu_core::ReadingDirection`, …) are unchanged — `lib.rs` re-exports these from
`view_modes` rather than `settings`.

### reading_progress

`reading_progress.rs` (`total` lifted to `Option<usize>` in #65). Transient, immutable core
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

`library.rs` (signature retyped in #65; book ordering added in #82; per-book view overrides added
in this feature).
`Library::register_opened(canonical: &Path, page_count: Option<NonZeroUsize>) -> OpenRegistration
{ resume: ReadingProgress, count_changed: bool }` centralises the open-time domain rule that
previously lived in `main.rs`'s `app::OpenBookUseCase::run`: idempotent add by canonical path
(dedup); page-count back-fill applied only for `Some(_)` (an unknown total = `None` is skipped);
resume lookup via `Book::progress()`. The positivity that was once enforced with a runtime guard is
now a type fact — `set_page_count(_, count: NonZeroUsize)` makes `0` unrepresentable at the write
boundary, so there is no `debug_assert` in core and no `page_count > 0` guard at the call site
(#65). The reader side maps stored counts through `Book::page_count_opt() -> Option<usize>`
(stored `0 → None`), the accessor that `progress()` and `carousel_data` consume. `main.rs` now
just calls `register_opened` and `jump_to(reg.resume.reached())`, converting at the boundary with
`NonZeroUsize::new(page_count)` (a zero-page open → `None` → back-fill skipped — though since the
reject-empty-books feature the open path bails out at `EmptyBookRemoved` BEFORE `register_opened` for
a zero-page source, so this `None` arm is now a defensive type-honesty wrapper rather than a live
path; see ADR-0009), keeping the
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

`Library` also holds `last_opened: Option<PathBuf>` (private, `#[serde(default, skip_serializing_if = "Option::is_none")]`): the canonical path of the most recently opened book, or `None`. Invariant: when `Some`, the path is present in `books`. Set by `register_opened` immediately after the idempotent add; cleared by `remove`/`remove_many` when they remove the matching book; orphan entry (path no longer in `books`) cleared by `normalize()` on load. Public accessor: `last_opened() -> Option<&Path>`. Persisted by the existing `save()` leave points; no separate save — same recipe as `last_page`/`overrides`.

Each `Book` also carries a `ViewOverride overrides` field (the per-book view-preference overrides;
`#[serde(default, skip_serializing_if = "ViewOverride::is_empty")]`, so an all-`None` override emits
no key and `library.json` stays byte-compatible with files written before this feature — see
[conventions.md](conventions.md), "Add an optional nested config"). The aggregate owns the mutation:
`Library::set_overrides(path, ViewOverride) -> bool` (idempotent-changed bool, same `false`==no-op
convention as `set_last_page`/`set_page_count`) and `Library::overrides_for(path) -> ViewOverride`
(returns `ViewOverride::none()` for an unknown path, so the caller can `resolve` unconditionally).
There is NO `Book` setter — `Book::overrides()` is read-only.

The bulk-delete rollback primitive (#129) is `Library::restore(books: Vec<Book>)` —
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

Navigation backed by `ImageCache`; drives a two-page spread with `apply` moving in spread units.
`open_path(path)` dispatches via `ArchiveLoader` + skipped warn + a `last_open_skipped()` getter,
and `open_folder` delegates to it. `jump_to(page) -> bool` — routes through
`SpreadContext::normalize` (via `spread_ctx()`) so `index` stays a valid spread leading, clamps
out-of-range, guards `page_count==0` to avoid underflow, and returns whether it moved, mirroring
`set_viewport_size`'s "did it change → caller refreshes" convention — and
`current_source() -> Option<Arc<dyn PageSource>>`
retaining the opened `Arc` because `ImageCache` does not expose its source; `index()`/`page_count()`
lost their `#[allow(dead_code)]`, now used by the thumbnail-strip wiring.

Production page delivery goes through two non-blocking methods: `spread_slots() -> Option<SpreadSlots>`
classifies the current spread into per-slot HIT (`Some(Arc<DecodedImage>)`) / MISS (`None`) pairs
without ever reading or decoding on a miss; `dispatch_handle() -> Option<CacheDispatch>` returns the
`Send` decode handle. `main.rs::refresh` calls `spread_slots()` and branches: all-HIT → apply
images synchronously; any-MISS → set `leading-loading` / `trailing-loading` loading flags and hand
the `SpreadSlots` to `PageController::dispatch_spread` for off-thread decode. `current_spread()`
(decodes synchronously on miss, surfaces the spread images as `SpreadImages`) is `#[cfg(test)]`-only
and never called on the UI thread in production.

The settings dialog drives IDEMPOTENT value setters `set_spread_mode(SpreadMode)`/`set_cover_mode(CoverMode)`/`set_reading_direction(ReadingDirection)`
(all `-> bool`, same value → `false` no-op, mirroring `jump_to`'s "moved? → caller refreshes"
convention) — `set_spread_mode`/`set_cover_mode` call `renormalize_index`,
`set_reading_direction` does NOT (pairing is direction-agnostic) — plus
`set_cache_config(cache_size, preload_pages)` which updates the fields `set_source` reads on the
NEXT open (the dialog's cache/preload edits would otherwise only take effect after relaunch, since
the fields are seeded once at `from_settings` and `set_source` reads ViewerState's own fields, not
live `Settings`).

`open_file() -> Option<&Path>`: the CANONICAL path of the most recently successful
`open_path` (set via `path.canonicalize().unwrap_or(verbatim)`, matching `Library::add`'s policy;
`None` until the first `Ok`, reset by `set_source`, unchanged by a failed `open_path`).
`view_sync.rs` reads it to form the write-back tuple `(canonical_path, index())` for the `Library`
at every leave point — see the resume/write-back scope entry and `docs/patterns.md`.

This-feature added `apply_resolved_view(ResolvedView)` — sets `reading_direction`/`spread_mode`/`cover_mode`
from a resolved view (the `fit_mode` field is applied by the caller via `ViewportState::set_fit`),
called after the source is set when opening/returning to a book so its per-book override (resolved
against global `Settings`) takes effect. Runtime modes are intentionally NOT reset on open — they are
re-seeded by this call — which is why the open-time global reconcile is a clobber trap (see
[patterns.md](patterns.md), "Write-direction invariant audit").

Two pure scrubber-support helpers:

- `scrub_fraction_to_page(fraction, page_count, rtl)` — pure, total, RTL-aware mapping of a
  `0..1` knob fraction to a raw 0-based page index (clamped, non-finite-safe). As of #71 it is the
  SINGLE LIVE source of that mapping: `Scrubber.slint` passes the raw clamped fraction up via
  `preview(float)`/`commit(float)` and `handlers/viewer.rs`'s `on_scrub_preview`/`on_scrub_commit` call this
  helper to resolve the page (the former in-Slint `drag-page` rounding is gone, so it no longer
  carries `#[allow(dead_code)]`). See [patterns.md](patterns.md) for the one-authoritative-side rule.
- `preview_is_double(page)` — returns whether a previewed page would land on a 2-page spread
  (using the same layout resolution the body uses) WITHOUT advancing the index; used by the
  scrubber preview to choose 1 vs 2 popover thumbnails.

`close(&mut self)` (#129): drops the cache and source, zeroes `page_count`/`index`,
and clears `open_file` — returning `ViewerState` to the no-book-open state. Display modes
(`reading_direction`/`spread_mode`/`cover_mode`) and `cache_config`/`viewport_aspect` are
deliberately preserved (closing a book is not a settings reset). Called by
`RemoveBooksUseCase::run` when the open book is among the deleted ones; `RemoveBooksUseCase::run`
itself also calls `ui.set_current_book_name("".into())` (`app.rs`~:481). The UI wiring sets this
name at boot and on a successful open via `current_book_name` from `view_sync.rs`.

### ViewportState

`viewport.rs`. UI-layer mutable zoom/pan/fit + viewport size; delegates ALL clamping to core pure
fns.

### keymap

Direction-aware key token → `KeyCommand` — page turns, D/R/C mode toggles, plus
direction-independent zoom/fit commands and `ToggleThumbnails` on `"t"`,
direction-INDEPENDENT like the zoom/fit keys.

### navigation

`navigation.rs`. Top-level screen state machine: `Screen { Library, Viewer }` enum +
`NavState` (private `screen` field, intent-named `to_library`/`to_viewer` transitions, boots to
`Library`; same private-field+intent-method convention as `ViewportState`). Free fns
`screen_to_index`/`index_to_screen` map the enum to/from the Slint `ViewerWindow.screen` int
property — contract: Library = 0, Viewer = 1. `screen_to_index` is an exhaustive match (a new
variant is a compile error); `index_to_screen` clamps out-of-range to `Library`.

`main.rs` holds the single `NavState` in `Rc<RefCell<…>>`; `go_to_library`/`go_to_viewer` seam
functions are the single chokepoints that flip `NavState`, push `screen` to the UI, and restore
focus. The Up arrow (`KeyCommand::GoToLibrary`, direction-independent) and the carousel
`open`/`move`/`back` callbacks route through them. `go_to_library` also rebuilds the carousel via
`refresh_library_carousel` (so the `BookmarkRibbon` reflects the current `last_opened`) and
one-shot-snaps the focused index to the last-opened book's visible row (resolved through the search
projection; falls back to 0 when `None`, filtered out, or empty).

### app

`app.rs`. `OpenBookUseCase` — the open-a-book application use case extracted from `main.rs` in
#67. Holds six shared collaborators as `Rc<RefCell<…>>` fields: `ViewerState`, `Settings`,
`ViewportState`, `Library`, `ThumbnailController` (`Rc`), and `CoverController` (`Rc`). There
is NO `NavState` field — `NavState` stays in `main.rs`. The return value (#114) is built from two
types and a single export:

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

The reject-empty-books feature added a third `OpenOutcome` variant: `EmptyBookRemoved { title,
removed, save_error }`, returned when the source opens CLEANLY but counts zero pages. `run` bails out
HERE — before `register_opened`, with the recents push / settings save / per-book view resolve /
carousel rebuild / thumbnail start all bypassed (so an empty book never re-enters via
`register_opened`); it removes the book if present (`Library::remove`, idempotent bool), re-saves,
best-effort purges the removed book's cover (via the shared `app::remove_empty_book`; Wave-2 #150
closed the old open-path purge gap), and pre-captures any save error. The recents push + settings save are DEFERRED past this check so
they fire only for a non-empty book (see [patterns.md](patterns.md), "Insert a guard before X").
`title` prefers the stored `Book::title`, falling back to `gashuu_core::display_title` (the core
derivation rule made public in Wave 1 #149; the Wave-2 #150 switchover dissolved the UI replica —
see [patterns.md](patterns.md), "Replicate-and-PIN" for the pattern's endgame). `finalize_open` handles the variant
by rebuilding the carousel (the active search filter preserved) and showing the notice ONLY when
`removed == true`; it does NOT switch screens, and the carousel-open / continue-reading sites guard
their `go_to_viewer` with an `enter_viewer` check so the user stays on a refreshed Library. The
open-time and cover-time removal paths share `main.rs::empty_book_removed_status` for the
notice-plus-save-failure compose.

`main.rs` constructs one instance and shares it (`Rc`) into the open-handler closures:
`on_open_folder`, `on_open_archive`, `on_carousel_open`, and `on_carousel_continue_reading`. All call
`finalize_open(&ui, &state, &viewport, &CarouselRefresh { … }, outcome)` after `run` returns — the
signature gained the full `CarouselRefresh` deps (was just `&localizer`) because the
`EmptyBookRemoved` arm may rebuild the carousel. The carousel-open / continue-reading sites also call
`go_to_viewer`, but BOTH guard it with `enter_viewer = !matches!(outcome, EmptyBookRemoved { .. })`
so an auto-removed empty source leaves the user on a refreshed Library instead of an empty viewer.
See [patterns.md](patterns.md), "OpenOutcome pattern" and "finalize_open helper".

Lives in the UI crate because it coordinates Slint components; `gashuu-core` is untouched.

`run` writes back the OUTGOING book's per-book view override through the persistence chokepoint —
`persist_view_modes(ViewModeRoute::OpenDifferentBook, …)` right beside the position write-back at
its top — and that route writes ONLY the per-book sink, so — per the per-book-overrides feature —
it does NOT reconcile runtime modes into the GLOBAL `Settings` on its open-time save (the runtime
still holds the outgoing book's per-book modes there; reconciling would clobber the global
defaults — see [patterns.md](patterns.md), "Write-direction invariant audit"). It then applies the
just-opened book's `ResolvedView` via `ViewerState::apply_resolved_view` (+ `ViewportState::set_fit`).

The parallel destructive use case (#129) lives in the same module:

- `RemoveBooksUseCase` — the "remove the selected books" use case. Holds four shared collaborators
  as `Rc<RefCell<…>>` fields: `ViewerState`, `Library`, `LibrarySearchState`, and
  `LibrarySelectionState`. `run(&self, ui) -> RemoveOutcome` executes the full destructive
  transaction in the non-negotiable order: snapshot selected paths → mutate+save with rollback
  (via `remove_books_with_rollback`) → best-effort cover purge via `cover_loader::purge_cover`
  (the single home of the cover-key recipe; `mtime_secs` and `COVER_MAX_SIDE` are private to
  `cover_loader.rs`; `tracing::warn`-only on miss) → clear the viewer via
  `ViewerState::close()` and blank the title bar when the open book was deleted → recompute the
  search projection → clear the selection (success only; `SaveFailed` preserves it so the user can
  retry). Returns `RemoveOutcome::NoSelection` (empty selection guard), `RemoveOutcome::SaveFailed
  { error }` (shelf rolled back byte-identically via `Library::restore`; selection preserved), or
  `RemoveOutcome::Removed { n, closed_open_book }`. The caller (`handlers/library.rs`'s `wire_selection_handlers`) rebuilds the carousel,
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

### view-mode persistence seam (`view_sync.rs`)

`view_sync.rs` owns `ViewModeRoute`, `persist_view_modes`,
`apply_global_view_to_runtime`, `current_book_name`, and `write_back_position`; the crate root may
re-export these `pub(crate)` seams so `app.rs` and `handlers/*` imports stay stable. It also keeps
`reconcile_settings`, `position_to_write_back`, `view_override_to_write_back`, and
`write_back_view_override` private. `persist_view_modes(route, &state, &viewport, &settings,
&library)` (peer of `write_back_position`) is the ONE chokepoint that routes runtime view modes to
their sink: `DialogClosedOnLibrary` → global reconcile; `DialogClosedOnViewer` / `LeaveViewer` /
`OpenDifferentBook` → per-book write-back; `AppExit` → per-book write-back FIRST, then a global
reconcile ONLY when `open_file().is_none()`. So the GLOBAL sink is reached only via the
Library-dialog close and the no-book-open exit; every leave point hits the per-book sink (a no-op
when no book is open). `apply_global_view_to_runtime(&settings, &state, &viewport)` mirrors the
GLOBAL `Settings` view modes into the runtime so the Library-screen settings dialog seeds from
global (the inverse of `reconcile_settings`). See [patterns.md](patterns.md), "Per-book view
overrides: write-back-at-leave-point + screen-scoped dialog routing".

### library_model

`library_model.rs`. PURE (Slint-free) `Library` → carousel display-row mapping: a plain
`CarouselData` struct (`title`/`current`/`total`/`progress`/`available`) + `carousel_data(&Library)
-> Vec<CarouselData>` in natural title order (inherited from `Library::books()` — no independent
sort here; see `Library aggregate` in the core section). The carousel counterpart of `thumbnail_strip`'s row mapping —
keeps the derivation table-testable without a display backend. Each row is built from
`Book::progress()`: 1-based `current = ReadingProgress::current()` (`reached + 1`, saturating);
`progress = ReadingProgress::fraction()` (guarded so an unknown/zero total → `0.0`, overshoot clamps
to `1.0`); `total = ReadingProgress::total()` (now `Option<usize>`, #65). The free derivation no longer lives in `library_model`
— it is centralised in `ReadingProgress` (see core entry below). `available` via
`Library::is_available`. `bookmarked: bool` is a pure derivation computed in
`carousel_data_for_indices`: `book.path() == library.last_opened()` — true for at most one row
(the last-opened book); the `BookmarkRibbon` atom in `CoverCard` shows when this is `true`.
`carousel.rs`'s `to_carousel_item` adapter builds the `!Send`
`slint::Image` (a `slint::Image::default()` placeholder, filled in asynchronously by `CoverController`)
on the UI thread; `build_carousel_model` is the build+bind
chokepoint returning the `Rc<VecModel>` so the cover loader and the library-add path mutate the
same model. `total` comes from the persisted `Book::page_count`: `0` until the count is known
(`progress` guarded to `0.0`), the real saved count afterwards — known by opening the book; for a
never-opened book added BEFORE the reject-empty-books feature, by `CoverController`'s background
page-count prefetch (see `cover_loader.rs`), which streams the count into this `total` and persists
it; for a book ADDED after that feature, persisted at add time by `apply_outcomes` (so a fresh add shows
its real `total` without waiting for the prefetch — see ADR-0009). The `total: clamp_to_i32(total)`
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

Bulk-delete feature (#126/#128), `library_model.rs` (pub(crate) struct). Owns the active
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
`confirm_delete_content`). Used as a collaborator by `RemoveBooksUseCase` (#129).

### carousel

`carousel.rs`. UI-thread adapter layer between `library_model` and Slint: `to_carousel_item` (private) maps a `CarouselData` row to a `CarouselItem` (placeholder `slint::Image::default()` cover); `build_carousel_model(library: &Library, indices: &[usize])` (pub(crate)) is a HEADLESS builder — no `ViewerWindow` arg — returning the `Rc<VecModel<CarouselItem>>`; a separate `bind_carousel_model(ui, model)` performs the Slint bind (the build/bind split enables unit tests over visible-index order); `cover_requests(library: &Library, indices: &[usize])` (pub(crate)) derives the per-book `CoverRequest` list, re-basing each request's `row` to the enumerated position in the filtered `indices` slice (not the library index), so cover targets stay aligned with the filtered carousel model; `thumb_state_at` (pub(crate)) re-fetches a row's thumbnail state for the scrubber preview.

### carousel refresh / projection (`carousel_refresh.rs`)

`carousel_refresh.rs` (extracted from `main.rs`, mirroring the `view_sync.rs` split). Owns the carousel-refresh/projection cluster: `refresh_library_carousel` (the single chokepoint that rebuilds + binds the filtered carousel model, optionally resets focus, re-applies the path-keyed selection, and (re)starts focus-prioritized cover loading), the `CarouselRefresh` borrowed-collaborator bundle it takes (`library` / `covers` / `search` / `selection` / `localizer`, all `pub(crate)`), the visible-index projection helpers (`visible_index_to_path`, `visible_focus_index_for_path`, `entry_focus_index` (private), `snap_carousel_focus_to_last_opened`, `clamp_focused_index`), and `push_selection_strings` (the selection-toolbar string chokepoint). UI-thread only; driven almost entirely from `handlers/library.rs` and `handlers/settings.rs`, with `go_to_library`/`go_to_viewer` (still in `main.rs`) routing their carousel work through it via the crate-root re-exports.

### enum_adapters

`enum_adapters.rs`. The 10 `pub(crate)` enum↔index adapters (8 were previously inline in `main.rs`): `reading_direction_to_index`/`index_to_reading_direction`, `spread_mode_to_index`/`index_to_spread_mode`, `cover_mode_to_index`/`index_to_cover_mode`, `fit_mode_to_index`/`index_to_fit_mode`, and the i18n pair `language_to_index`/`index_to_language`. Each `index_to_*` clamps out-of-range to the first variant, mirroring the `index_to_screen` clamp policy in `navigation.rs`.

### i18n

Fluent i18n (#112), `i18n/` module (`mod.rs` + `loader.rs` + `dynamic.rs`). **`messages.rs`
was deleted (#114)** — all runtime-composed strings now live in `i18n/dynamic.rs` (see
below). `Localizer` wraps an `i18n_embed::fluent::FluentLanguageLoader`; its `new(lang)`/`switch(lang)`
call `load_languages` then re-apply `set_use_isolating(false)`, and `panic!` on a load failure
(compile-time-embedded assets ⇒ programmer error — see [patterns.md](patterns.md), "Fluent loader").
All mutating methods take `&self` (the loader has interior mutability), so `main.rs` shares one
`Rc<Localizer>` into the Slint callbacks without a `RefCell`. The `pub(crate) fn loader()` getter is
justified by `dynamic.rs` as the real consumer (per the dead-code-getter rule in
[patterns.md](patterns.md), it was deferred until the getter had a non-test caller). `loader.rs`
holds the `#[derive(RustEmbed)] struct Localizations` (embeds `i18n/`) and the exhaustive
`langid_for(Language) -> LanguageIdentifier` (no wildcard — the compile-time gate replacing the
former `messages.rs` exhaustive match). The completeness/parity/byte-oracle integration tests live
in `mod.rs`'s `#[cfg(test)]`. The `Language` enum stays in headless `gashuu-core`; ALL Fluent
machinery is UI-crate-only, per [ADR-0002](ADRs/0002-layered-two-crate-architecture.md).
`Localizer::apply(&self, ui: &ViewerWindow)` (#113) in `mod.rs` is the single chokepoint that resolves
every Fluent-served static string via `fl!()` and pushes it into the `Strings` global (next entry);
`main.rs` calls it at boot; after each `switch()` (language change) the call is in
`handlers/settings.rs`'s `wire_view_mode_handlers` — see [patterns.md](patterns.md), "The
`Strings`-global push".

**`i18n/dynamic.rs`** (Fluent i18n, #114): Fluent-backed dynamic message functions that
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

### handlers

`handlers/` module (#151). Slint callback registration, split by feature area. `mod.rs` declares
the three sub-modules and re-exports all eight `wire_*` fns at the `handlers::` level so `main.rs`
needs no sub-module path. Each `wire_*` fn takes `&ui` and exactly the `Rc` handles its closures
clone — the per-closure `Rc::clone` list IS that handler's dependency list (#151 panel constraint:
no AppState bundle, explicit handle lists only). The three feature files are:

- **`handlers/library.rs`**: `wire_open_handlers` (open-folder, open-archive, add-books,
  add-folder), `wire_carousel_handlers` (carousel search/open/continue-reading/move/back),
  `wire_selection_handlers` (toggle/cover-click/select-all/exit selection; bulk-delete confirm +
  confirm-accepted; empty-book-detected auto-removal). Also constructs the `RemoveBooksUseCase`
  instance inline (the use case's full collaborator list is only available here) and owns the
  `on_empty_book_detected` UI thread handler.
- **`handlers/settings.rs`**: `wire_settings_handlers` (settings dialog open/close, shortcuts
  overlay open/close, reset-overrides, first-run guide dismissal),
  `wire_view_mode_handlers` (view-mode setter callbacks — reading direction, spread, cover, fit,
  language, cache/preload/track; the language arm calls `localizer.switch` then `apply`).
- **`handlers/viewer.rs`**: `wire_viewer_input_handlers` (thumbnail click, page-jump, chrome
  reveal, scrubber preview/commit, thumbnail-strip toggle), `wire_viewport_handlers` (viewport
  size + pan/zoom callbacks), `wire_nav_handlers` (keyboard nav hub: page turns, mode toggles,
  GoToLibrary, window resize).

`fn main` = boot (tracing, settings/library load, Slint window, localizer, Rc construction, seed
carousel, prune) + 8 wire calls + `ui.run()` + exit flush (count persistence, write-back,
view-mode persistence, settings save). All callback closures live in `handlers/`; `main.rs` retains
`refresh`, `finalize_open`, `go_to_library`/`go_to_viewer`, and the add-batch helpers, plus the
crate-root re-exports for the `view_sync.rs` and `carousel_refresh.rs` seams (the carousel-refresh
/projection cluster itself now lives in `carousel_refresh.rs`).

### Slint UI files

**`ui/components/`** (#71, NEW): shared single-purpose UI atoms/molecules, one `export`ed component
per file — `ProgressBar` (accent/`success` reading-progress fill), `PrimaryButton` (the accent CTA),
`ThumbnailCell` (the loaded/loading/failed/highlighted
cell shared by the page strip, the scrubber preview popover, and the library covers), `ViewerPill`
(the viewer glass-pill — floating page-jump field + thumbnail toggle + settings;
replaces the docked TitleBar), `NavBar`
(#83, NEW: the top-centered glass-pill Library nav — translucent fill + hairline border + top inner
highlight + drop shadow; Slint has no backdrop-blur so the glass effect is paint-only), `NavItem`
(#83, NEW: one circular icon capsule inside `NavBar`; hover/press glow via `Theme.accent-glow`;
non-focusable `TouchArea` with `accessible-role`/`accessible-label`/`accessible-action-default` for
screen-reader support; `feat/library-chrome-polish` added two opt-in props: `in property <bool> active: false` — holds a persistent accent ring + `accent-glow` fill, priority pressed > hover > active; and `in property <bool> enabled: true` — gates the `TouchArea` and `accessible-action-default`, drops the icon to `text-faint` when false; existing call sites are untouched via their defaults), `Dropdown` (i18n feature, NEW: the Apple-HIG pull-down button used by the
language setting — the repo's first `PopupWindow`; the menu is lowered to the window root so the
dialog's clipping Flickable can't cut it off — see docs/patterns.md), `Segmented` (issue 102:
token-driven horizontal segmented control replacing the std-widgets `ComboBox`; equal-width
cells with an accent-pill selected state, keyboard Left/Right navigation), `Stepper` (issue 102:
token-driven integer stepper replacing `SpinBox`; surface-sunken capsule with accent ± glyphs,
keyboard Up/Down), `Toggle` (issue 102: token-driven on/off switch replacing `CheckBox`; pill
track accent-on / `track-prog`-off, spring-animated knob), `SettingRow` (issue 103: L1
alignment molecule — fixed label column + `@children` control slot spanning to the shared right
rail; `trailing` flag pushes compact atoms onto the rail via a leading stretch spacer),
`SelectionBadge` (bulk-delete feature; two-state added in the UI-polish pass: pure visual atom overlaid
on EVERY cover while selection mode is active — `checked == true` renders the original accent-filled
disc + centered white check glyph; `checked == false` renders a hollow hairline ring over a faint
glass backing so unselected covers are clearly afforded as targets; both Carousel `for` passes
include it, gated on `selection-mode` not `selected`), `SelectionToolbar` (bulk-delete feature (#129);
UI-polish pass §2.6: selection-mode organism — count pill / select-all capsule / exit capsule /
**Delete (N)… `DangerButton`** (if-gated, hidden at N=0; the only red element in the app's chrome);
glass-pill following `NavBar`'s four-layer idiom with two intentional departures
(accent border-color for mode context; `active`-gated drop shadow to suppress bleed from the
parked position); content-hugged width; mounted ALWAYS inside a `clip: true` slide-strip below the
NavBar in `Carousel` (y-slide transition — NOT an `if`-gate so the animation is continuous; as of `feat/library-chrome-polish` 2026-06-05 this strip is toolbar-only — the "Select" entry pill that previously shared the strip was replaced by the NavBar Select capsule); takes
an `active` flag ANDed into every `TouchArea`/a11y guard; all human-visible text passed from Rust
via `in` properties — deliberately `Strings`-agnostic), and `DangerButton`
(bulk-delete feature: destructive-action atom — structural clone of `PrimaryButton` swapping accent for
`Theme.danger` red + `Theme.danger-glow` hover/focus ring; accessibility role/label/action-default
added here, `PrimaryButton` backport pending; consumed by the `ConfirmDialog` and the
`SelectionToolbar` delete button), and `BookmarkRibbon` (continue-reading feature:
display-only Image atom floating ABOVE the cover's top-LEFT corner, `@image-url("../assets/bookmark.svg")`
recolored via `colorize: Theme.text` white — a quiet non-interactive status color (not accent, which is reserved for interactive elements; `feat/library-chrome-polish` swapped from accent + tucked-in to text-white + fully detached); placement `x: Theme.space-xs; y: -self.height - Theme.space-md` (~10 px clearance ≈ ribbon height / φ²; zero cover overlap; dark stage guarantees contrast); sized `Theme.space-huge²`; shown when `CarouselItem.bookmarked` is true — the single last-opened book resolved from `Library.last_opened`; accessible as `role: image` / label `Strings.continue-reading`; the ribbon sits outside the card above-left while `SelectionBadge` sits inside the card top-right; both can render simultaneously).
Each references `Theme.*` via `../Theme.slint`;
consumers import via `import { X } from "components/X.slint"`. `build.rs` is unchanged — it compiles
the single entry `ui/ViewerWindow.slint` and import statements cascade. See
[docs/conventions.md](conventions.md) for the component RULES.

**`ui/assets/`** (#83, NEW): the repo's image assets — `file.svg`, `folder.svg` (#83), `plus.svg`
(the macOS combined "Add books" capsule glyph), `carousel.svg`
(the thumbnail-toggle icon in ViewerPill; replaced the original `slider.svg` glyph),
`chevron-down.svg` (i18n PR, NEW: the `Dropdown` chevron; 96px intrinsic size per the HiDPI
rasterization rule), `check.svg` (bulk-delete feature, NEW: the single-path check mark recolored
via `colorize` to `Theme.text` white inside `SelectionBadge`), `close.svg` (UI-polish pass, NEW:
Cancel Fill glyph — disc + knocked-out ✕ — used by the `SelectionToolbar` exit capsule, recolored
via `colorize`; replaced the former `Text` pseudo-icon), `delete.svg` (UI-polish pass, NEW:
trash glyph shown as the `DangerButton` leading icon in `SelectionToolbar`, recolored via
`colorize`), `filter.svg` (`fix/ui`, Streamline "Filter Fill" — the NavBar Select capsule glyph; replaced the original `checkbox.svg` "Check Box Fill" from `feat/library-chrome-polish`; still a solid shape, chosen to survive 21px femtovg rendering where dashed marquee glyphs blur; recolored via `colorize`), and `bookmark.svg` (continue-reading feature: Streamline "Bookmark Check Fill" glyph since `feat/library-chrome-polish` — same SVG install point, glyph swapped from plain Bookmark Fill to Bookmark Check Fill; 96×96 intrinsic / viewBox 24; consumed by `BookmarkRibbon.slint` recolored to `Theme.text` white, and by the NavBar bookmark capsule via `@image-url("assets/bookmark.svg")` in `Carousel.slint`), each a
single-path SVG recolored at runtime via Slint's `Image.colorize` property. Components reference
them with `@image-url(...)` paths relative to the consuming `.slint` file. `build.rs` is unchanged
(assets are reached transitively through the entry-file import cascade).

**`ui/Strings.slint`** (Fluent i18n, #113, NEW): `export global Strings` — the Fluent-served
static-string surface, 67 `in property <string>` slots (property name == Fluent message ID) with
English-literal defaults. Written exclusively from Rust by `Localizer::apply()`; `.slint` bindings
read `Strings.<prop>` in place of the removed `@tr()` calls. `ViewerWindow.slint` re-exports it
(`import` + `export { Strings }`) so Slint generates the `ui.global::<Strings>()` accessor. See
[docs/patterns.md](patterns.md), "The `Strings`-global push".

**`Carousel.slint`** (#71 componentized; #83 glass-pill nav): Library
cover-flow carousel.
The public contract is the `CarouselItem` struct + `Carousel` component with `items`,
`focused-index`, callbacks `open(int)`/`move(int)`/`back()`, `public function focus-self()`. The
rendering against that contract: centered focused cover (accent ring) +
scaled/dimmed neighbors, a per-cover `ProgressBar` (#71 promoted the former file-private bar to the
shared `components/ProgressBar.slint`, reused for the focused-meta bar), a centered focused-book meta
block, a grayed broken-cover placeholder for
unavailable books, and the 0-book empty-state CTA. #71 also routes its covers and empty-state CTA
through the shared `ThumbnailCell`/`PrimaryButton` components. Covers start as placeholders
(`slint::Image::default()`); `cover_loader.rs` streams the real cover images into the same
model row-by-row. The add callbacks (today `add-books()`/`add-folder()`; `add-books` was
named `add-files` until the macOS combined picker landed) wire the empty-state CTA
to `add-books()` (each restores focus via `focus-self()` after firing); the original left-aligned
two-`Button` text toolbar was REPLACED in #83 with a centered, icon-only glass-pill `NavBar` (two
`file`/`folder` `NavItem` capsules on Windows/Linux; on macOS a single combined `plus` capsule,
gated by the Rust-pushed `combined-add-picker` flag). `feat/library-chrome-polish` extended the NavBar with two additional capsules: a **Select capsule** (`filter.svg`; API: `select-icon`, `selection-active`, `select-enabled`, `toggle-selection()`) that toggles bulk-selection mode (disabled — not removed — when the library is empty; modal-input blocked by the existing backdrop absorber like all other NavBar controls); and a **bookmark capsule** (`bookmark.svg`; API: `bookmark-icon`, `continue-reading()`) that jumps to the continue-reading book — always enabled, using the status strip to respond when no bookmark exists. Capsule order: `[search | add capsule(s) | select | bookmark | settings]`; width formula was updated atomically in both `combined` and non-`combined` branches (4/5 capsules, 5/6 gaps). The `NavBar` is declared as a SEPARATE LAST layer in the
component tree — paint order equals declaration order in Slint, so it renders on top of the cover-flow
without a z-index — and kept OUTSIDE the `FocusScope` so keyboard navigation remains carousel-owned;
the nav is mouse + screen-reader oriented. The `add-books()`/`add-folder()` callbacks and the
`focus-self()`-after-fire behavior are unchanged; `NavBar` simply forwards into them. The cover-flow
is rendered by a file-private `CoverCard` sub-component instantiated by TWO `for` passes over the model:
pass 1 is the always-on BACKING layer (`show: true` for every book), pass 2 (declared after) paints ONLY
the centered card (`Math.round(flow-position)`, the visual center of the animated row) so it draws ON TOP
of its backing twin — Slint 1.x cannot set per-`Repeater`-item z, so draw order is the only lever. The
Left/Right slide is driven by ONE animated float (`flow-position`, chasing `focused-index` on the
cinematic curve); every per-card value is a pure binding on it, so rapid input retargets a single
animation and the row moves as one band (docs/patterns.md "animation altitude"). Both passes bind
identical geometry (each book keeps a persistent instance in each pass), so the layer hand-off is
seamless. The enclosing row's `width`/`row-cy` are passed into `CoverCard` as `in` properties because a
component ROOT cannot read `parent`.

**`Theme.slint`** (NEW; #83 extended): single `global Theme` of visual tokens (colors, spacing,
radii, font sizes); components reference `Theme.<token>` instead of inline hex literals. #83 added the
glass-surface colors (`glass-fill`, `glass-border`, `glass-highlight`) and the golden-ratio nav sizing
tokens (`nav-icon`, `nav-capsule`, `nav-pill-height`, `nav-item-gap`, `nav-pill-pad`). Authoritative
values live in DESIGN.md; this file is the as-built note only.

**`ThumbnailStrip.slint`** (#71): horizontal `Flickable` + `HorizontalLayout` + `for` over a
`VecModel` — the FIRST `VecModel`/`Repeater` use in the codebase since `ListView` is
vertical-only — over `struct ThumbnailItem { image, page, loaded, failed }`. #71 replaced the inline
per-cell markup with the shared `ThumbnailCell` component (border ring / background / radius /
loaded-loading-failed-highlighted states now live there).

**`thumbnail_strip.rs`** (`ThumbnailController`, issue #30): owns the strip's
`Rc<VecModel<ThumbnailItem>>`, the epoch counter, and the cancel flag; `new(&ui)` builds and binds the
model via `ui.set_thumbnails`; `start(&self, ui_weak, source, page_count)` cancels any in-flight
generation, resets the model to `page_count` placeholders, and spawns the background worker. `main.rs`
constructs the controller once and calls `thumbs.start(...)` in every open handler, with no thumbnail
bookkeeping inline.

**`cover_loader.rs`** (`CoverController`): a structural
TWIN of `ThumbnailController` for the Library carousel. Streams each book's real cover into the shared
`VecModel<CarouselItem>`. `start` is DISPATCH-ONLY on the UI thread: every `CoverRequest` becomes one
`rayon::spawn` worker (`spawn_load`), and the worker does ALL the per-book I/O — derive
`cache_key(path, mtime, max_side)` (the mtime `fs::metadata` happens on the worker), try
`ThumbnailCache::get` (a HIT reads + decodes the cached PNG on the worker), or on a MISS open via
`ArchiveLoader`, call core `generate_cover`, `ThumbnailCache::put` — then `invoke_from_event_loop` to
set the row. Hit and miss share this one worker path (a warm 500-book start used to decode 500 PNGs
inline on the event loop), under the SAME epoch + cancel double-guard and Send/!Send discipline as the
thumbnail strip (see [patterns.md](patterns.md)). The cancel-rotation lives in a shared-shape private
`rotate_cancel(&self) -> Arc<AtomicBool>` helper kept IDENTICAL in both controllers. Requests are
dispatched focus-first: `prioritize_by_focus(requests, focus_row)` (pure, unit-tested) sorts by
`abs_diff` from the carousel's focused row so the visible neighbourhood streams in before off-screen
rows. Covers render at `COVER_MAX_SIDE = 512` px — DECOUPLED from the strip's
`DEFAULT_THUMB_MAX_SIDE = 160` (a focused cover slot is far larger than a strip cell, so 160 px upscaled
blurry); `max_side` is in the cache key, so raising it auto-regenerates stale 160 px covers. The
controller ALSO prefetches each unopened book's REAL page count (fixing the "1 / 0" display): a request
flagged `needs_count` (`Book::page_count_opt() == None`) resolves the count on the same worker — the
cover-MISS worker reuses its archive open (`source.list_pages().len()` before `generate_cover`), a
cover-cache HIT opens the archive once for the count AFTER marshalling the cover (so the count never
delays the visible image). `marshal_total` streams the count to the row's `total` for immediate display,
and a `ResolvedCount { path, count }` is queued in `pending_counts` for UI-thread persistence — drained
via `Library::set_page_count` + `save` at the next `start` and at shutdown (`flush_counts`), so the
count survives a relaunch (a worker can't touch the `!Send` `Rc<RefCell<Library>>`, hence the queue).
Callers build the `CoverRequest` list BEFORE `start` so the `library.borrow()` is released before
`start`'s persistence `borrow_mut`. The reject-empty-books feature added empty-book DETECTION to the
worker: a worker that opens a book CLEANLY but counts zero pages (the pure `should_signal_empty`
decision = `open_result.is_ok() && count == 0`) calls `marshal_empty_book`, which fires the root
`empty-book-detected(string)` Slint callback under the same epoch guard as `marshal_total` (the
worker holds only a `Send` `PathBuf`; the `!Send` removal work runs on the UI thread in
`handlers/library.rs`'s `on_empty_book_detected`). An open ERROR is unreadable, NOT empty — it keeps the placeholder
+ log behavior and fires no signal. A dropped stale-epoch signal cannot lose the detection (a zero
count is never persisted, so the next generation re-detects), and a removed book is absent from the
next generation's requests (no loop). See [patterns.md](patterns.md), "Worker → UI ACTION via a
Slint callback", and [ADR-0009](ADRs/0009-reject-empty-books.md).

**`add_loader.rs`** (`AddController`, issue 206): the SAME async harness applied to the bulk ADD, so opening
several large/cloud-synced archives in the `+` picker never freezes the event loop (`probe_page_count` reads each
ZIP's central directory; `File::open` can block on hydration). `start(ui_weak, paths, policy, op)` bumps an
`AtomicUsize` epoch, installs a fresh `Arc<Mutex<Vec<ProbeOutcome>>>` accumulator, shows `Adding… (0/N)`, then
dispatches one `rayon::spawn` per path running the pure, `Send` `probe_path` (classifies into
`ProbeKind::{Counted(NonZeroUsize), Empty, FormatDisabled, Unreadable}` — touching NO `Library`). Each worker
pushes its `ProbeOutcome`, marshals an epoch-guarded `add-progress(done, total)` tick, and the worker that drains
the `remaining` `AtomicUsize` to zero marshals `add-finalize(epoch)`. The finalize handler
(`handlers/library.rs`) drains via `AddController::take_outcomes(epoch)` — epoch-guarded, so a second add started
mid-probe supersedes the first with no stale clobber — re-sorts to INPUT order, then runs the APPLY half on the UI
thread: `main.rs::apply_outcomes` (the old synchronous `add_paths` body, byte-identical behaviour, logging deferred
here so the probe stays pure) + `apply_add_report` (save → rebuild → notice → focus). Same Send/!Send discipline as
the cover loader: only `Send` values cross into the workers and the marshaled closures; every `Library` mutation
runs on the UI thread. See [patterns.md](patterns.md), "Worker → UI ACTION via a Slint callback".

**`page_loader.rs`** (`PageController`, issue #207): the viewer's async-decode arm. Keeps the same
mental model as `cover_loader.rs` — UI-thread bookkeeping in the controller, heavy decode on rayon
workers, scalar-only `Send` values marshalled back via Slint callbacks — applied to page turns so a
cache-miss page never blocks the event loop. `PageController` owns an `Arc<AtomicUsize>` epoch, a
`RefCell<HashSet<usize>>` dispatch-dedup set, and a `RefCell<Option<SpreadTarget>>` (holding
`leading_idx`, `trailing_idx`, `single`). `set_source` / `set_target` advance the epoch so stale
marshal closures that arrive after a source change or spread navigation are silently discarded.
`dispatch_spread(ui_weak, cache_dispatch, request)` reserves each MISS slot independently via
`reserve_missing_slots` (dedup prevents duplicate in-flight decodes for the same page); when both
slots are MISS, the rayon job uses `rayon::join` so they decode in parallel. Each rayon job marshals
back via `slint::invoke_from_event_loop`: on success it calls `ui.invoke_spread_anchored(content_w,
content_h, single, trailing_failed, leading_idx, trailing_idx)` (which drives geometry + image
apply on the UI thread); on decode failure it calls `ui.invoke_page_decode_error(index)`. The
UI-side `slint::Image` is built inside the event-loop closure (never `Send`). `main.rs::refresh`
calls `ViewerState::spread_slots()` to classify the current spread, branches on all-HIT vs
any-MISS, and hands a `SpreadDecodeRequest` to `dispatch_spread` on a miss.

**`SettingsDialog.slint`** (issue 102 replaced its std-widgets `ComboBox`/`SpinBox`/`CheckBox` with the token-driven `Segmented`/`Stepper`/`Toggle`/`Dropdown` atoms): modal overlay editing active settings; two-way `current-index <=> in-out-prop` +
`selected`/`edited`/`toggled` callbacks. Since issue 103/104 it is a **content-hug glass panel** (φ relocated into the component proportions; spec 2026-06-04) built from custom `components/` atoms; its footer "Shortcuts" link opens `ShortcutsOverlay`, and an `in property <int> focus-epoch` (bumped by `ViewerWindow.focus-settings()`) lets the parent re-focus this still-mounted dialog after the overlay closes.

**`ShortcutsOverlay.slint`** (issue 104, NEW): a second glass modal listing the keyboard shortcuts read-only, stacked OVER the still-mounted `SettingsDialog` (both `show-settings` and `show-shortcuts` true). Reuses the settings glass recipe (`settings-w`/`settings-radius` + all glass tokens; only `shortcuts-h` is new). Its ancestor `FocusScope` traps every key so focus can't leak to the dialog underneath; closing returns focus to the dialog via the epoch seam. The keyboard-shortcuts reference text documents the full selection grammar (#129): `x`, `Space`, `Cmd/Ctrl+A`, `Delete`/`Backspace`, `Esc`.

**`components/ConfirmDialog.slint`** (issue 127, NEW; wired in `ViewerWindow` for the bulk-delete path, #129): a GENERIC two-choice confirm/cancel modal — no domain vocabulary. Every string on screen arrives through `in` properties (`title`, `body-lines: [string]`, `info-text`, `warning-text`, `confirm-label`, `cancel-label`) so the same component is reusable across confirm decisions. Clones the `SettingsDialog` / `ShortcutsOverlay` glass idiom (scrim + four-layer fake-glass object). Mounted in `ViewerWindow` behind `if root.show-confirm-delete` (an `if`-gate so the node is constructed only when needed). Cancel / Esc / backdrop click fire the `cancel` callback (Slint-side: set `show-confirm-delete = false`, restore carousel focus — selection PRESERVED); the confirm `DangerButton` fires the `confirm()` callback (`ConfirmDialog.slint:117`), which `ViewerWindow` forwards as `confirm => { root.confirm-delete-accepted(); }` (ViewerWindow.slint:591), and Rust registers on `ui.on_confirm_delete_accepted` to run `RemoveBooksUseCase` and dismiss the modal. `Enter` is wired to Cancel (the destructive action is never on `Enter`).

**`FirstRunGuide.slint`** (NEW): dismissable once-only overlay; a local `GuideLine`
component dedupes the key-reference rows.

**`Theme.slint`** (NEW; completed #70): a single Slint `global Theme` that centralises all visual design
tokens — colors, corner radii, spacing, font sizes, component sizes, shadow colors, motion durations, and font weights —
sourced from `/DESIGN.md`. ALL UI components reference `Theme.*`; the three previously-inline-hex dialogs (ThumbnailStrip, SettingsDialog, FirstRunGuide) were migrated in #70, and `scripts/check-tokens.sh` is now unconditionally blocking for the whole UI (no allowlist).

**`PageView.slint`**: the page canvas; hosts pan/zoom via a single `TouchArea`.
A `reveal()` callback, fired on `changed mouse-x` / `changed mouse-y` (pointer-move),
triggers the auto-hiding viewer chrome.

**`Scrubber.slint`** (#71): bottom auto-hiding page-scrubber with a drag-time thumbnail
preview popover. Public surface: `in` properties `current-page` / `total-pages` / `rtl` /
`double` / `preview-a` / `preview-b` / `chrome-shown`; callbacks `preview(float)` / `commit(float)`
(#71 retyped these from `int`: the scrubber now passes the RAW clamped knob fraction and Rust owns
the fraction→page rounding — see [patterns.md](patterns.md)). Drag fires `preview` only;
pointer-release fires `commit`. Its preview thumbs use the shared `ThumbnailCell` component.

**`ViewerWindow.slint`**: hosts the two `if root.show-X : Component` overlays
(last children = front), a "Settings…" toolbar button, the in/in-out properties + setter
callbacks, and a FocusScope key-guard. The wiring (dialog/guide lifecycle) was originally registered in `main.rs` and has since
been extracted to `handlers/settings.rs`; the 8 enum↔index helper fns moved to `enum_adapters.rs`; the key-bindings help text is now composed via the localizer and pushed via `ui.set_key_bindings_text(…)` from `handlers/settings.rs`. It carries a two-screen model: `in property <int>
screen` gates the Library `Carousel` (screen 0) vs the Viewer body (screen 1) via
`visible: root.screen == N` (not `if` — see [patterns.md](patterns.md) for the Slint id-scoping
reason); Settings/Guide overlays remain viewer-scoped. It mounts the
`Scrubber` as auto-hiding chrome inside the screen-1 viewer,
driven by a `chrome-shown` bool + an idle `Timer`; chrome is revealed on pointer-move (via
`PageView.reveal()`) and scrubber drag — NOT on page turns (arrows / Space / Backspace / tap /
swipe stay quiet so the chrome doesn't flash on every turn). #71 mounted the shared `TitleBar`
component (bound to a new `current-book-name` in-prop — derived by `view_sync.rs`'s
`current_book_name` from the post-open `ViewerState::open_file()`, see [patterns.md](patterns.md)),
and set a `min-width`/`min-height` floor on the window. `feat/library-chrome-polish` added
`in property <string> library-count-text` (the pre-composed total book count for the Library
screen's bottom status strip idle state); the strip's `Text` resolves
`text: root.status-text == "" ? root.library-count-text : root.status-text` — transient notices
always win; and forwarded the Carousel's `continue-reading()` callback up to the window level for
Rust handling.

### rfd file/folder picker

`on_open_archive` → `rfd` `pick_file` filtered to cbz/zip/cbr/rar (the filter dispatch goes
through `open_path` via `ArchiveLoader`). "Open Archive" button lives in `ViewerWindow.slint`.

Library-side pickers: `on_add_books` (né `on_add_files`; filtered cbz/zip/cbr/rar —
`pick_files_or_folders` on macOS, where one NSOpenPanel picks archives AND folders in a single
panel, `pick_files` elsewhere; the platform split lives in `#[cfg]` blocks inside the one handler,
and the matching `combined-add-picker` bool pushed at boot collapses the NavBar's two add capsules
into one on macOS) and
`on_add_folder` (`pick_folder`, folder-as-one-book; its capsule is hidden on macOS). The library-add seam is split
across the UI-thread boundary (issue 206, to keep a bulk add of large/cloud-synced archives from freezing the event
loop): the PROBE half runs OFF the UI thread — `add_loader::AddController::start` dispatches one rayon
`add_loader::probe_path` per picked path (`ArchiveLoader::probe_page_count` → a `Send` `ProbeOutcome`, touching no
`Library`), streams `Adding… (k/N)` progress via the `add-progress` callback, and marshals `add-finalize` when the
last probe completes (an `AtomicUsize` epoch supersedes a second add started mid-probe). The APPLY half runs on the
UI thread in the `add-finalize` handler: `main.rs::apply_outcomes` (dedup-aware insert, rejects empty/unreadable
sources, persists the probed count on each genuine insert, returns `AddReport { added, skipped }` — see ADR-0009),
`build_carousel_model` (Library → `ModelRc<CarouselItem>`,
0-based `last_page` → 1-based `current`, real `total`/`progress` from persisted `Book::page_count`,
placeholder cover), and the shared
`apply_add_report` tail (save → rebuild carousel → status line → restore carousel
focus; short-circuits when nothing new was added). The 4-way add notice is chosen by the pure
`select_add_notice(added, skipped) -> AddNotice` selector (testable without Slint; see
[patterns.md](patterns.md), "Pure-selector seam"). The persisted `Library` lives in `main.rs` as
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
