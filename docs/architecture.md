# Architecture

This is the L2 reference document — the as-built MODULE MAP for the gashuu codebase. It was
migrated from the `## Architecture` section of `CLAUDE.md`. Architectural DECISIONS (the "why"
behind each choice) live in [docs/ADRs/](ADRs/README.md); this document is the current module
inventory: what exists, where it lives, and what it does.

---

## crates/gashuu-core (headless domain + I/O)

Slint-independent domain + I/O layer. See [ADR-0002](ADRs/0002-layered-two-crate-architecture.md)
for the rationale behind the two-crate split. Keep the layer boundary strict.

### page_decode

`PageDecoder` takes owned encoded bytes and returns a local `DecodeFuture` that resolves
to straight-alpha RGBA8 and dimensions. `ImageCratePageDecoder` wraps `image_ops::decode`
in a ready future. `decode_with_fallback` retries the same bytes after a primary failure.
A successful retry returns the image and the primary error. If both attempts fail, the
caller receives the fallback error.

### PageSource trait

Requires `Send + Sync` so `Arc<dyn PageSource>` can be shared with rayon worker threads during
prefetch. `skipped_count(&self)->usize` is a TRAIT method — default `0`, overridden by
`FolderSource`/`ZipSource`/`RarSource` so the `Arc<dyn PageSource>` from `ArchiveLoader` exposes
it uniformly and `MockPageSource`/future sources need no change.

`listing_truncated(&self) -> bool` sits beside it with the same defaulting discipline — `false` on
the trait, `false` for `FolderSource`/`ZipSource` (their walks are resumable), `true` only for
`RarSource` when a damaged header ended `unrar`'s non-resumable `List` iterator. Its meaning: the
returned pages are a PREFIX of the archive and an UNKNOWN number of later pages are missing. It is
strictly distinct from `skipped_count`, which counts individual entries dropped from an otherwise
COMPLETE listing — the two must never be conflated, because a cut-short listing makes the number of
lost entries unknowable.

See [ADR-0004](ADRs/0004-archive-abstraction-and-extraction.md) for the `PageSource` abstraction
decision. See [ADR-0011](ADRs/0011-decoder-subprocess-isolation.md) for the planned subprocess
isolation of RAR/CBR and AVIF decoders.

### FolderSource

Top-level directory walk with natural filename ordering (`max_depth(1)`, no recursion).

### ZipSource

`page_source/zip.rs` uses the synchronous `zip` crate and includes images at any archive depth.
`ZipSource::from_read_at` parses the central directory once and indexes the image pages.
Each `read_bytes` clones the parsed archive. The clone shares the directory metadata and has
its own cursor, so rayon prefetch threads can read and decompress entries in parallel.

`page_source/read_at.rs` provides `ReadAt`, a `Send + Sync` trait for reading bytes at an explicit
offset. `ReadAtCursor` implements `Read + Seek` over a shared source and stores a separate
position in each clone. Desktop `ZipSource::open` uses `FileReadAt` and holds one file handle for
the lifetime of the source. The `Arc<[u8]>` implementation also works on wasm32.

### RarSource

The default-on `rar` feature provides `RarSource`. Without it, opening a RAR/CBR returns `CoreError::FormatDisabled { format: "rar/cbr" }`.

`page_source/rar.rs`. A `PageSource` over a RAR/CBR archive using the SYNCHRONOUS `unrar`
crate (bundles C++ UnRAR built by `cc`; extraction ONLY — no Rust RAR encoder exists), flattening
nested images like `ZipSource`. Lock-free via reopen + sequential-skip (RAR has NO random access):
each `read_bytes` opens its OWN `::unrar::Archive` + `open_for_processing()`, then
`read_header()`/`skip()` walks forward to the target's `seq_index` before `read()`.

### entry_policy.rs

Shared image-extension recognition (`IMAGE_EXTS`, `has_image_ext`) and archive-entry path
validation (`MAX_ENTRY_BYTES`, `enclosed_name`) — all `pub(crate)`. `IMAGE_EXTS`/`has_image_ext`
were extracted from `folder.rs` so `FolderSource`/`ZipSource`/`RarSource` recognise images
identically. `enclosed_name` is the traversal/zip-slip guard rejecting absolute /
root-or-prefix / any `..` paths, mirroring `zip`'s protection for RAR entries; `MAX_ENTRY_BYTES`
lives here (neutral shared 500 MB archive-entry ceiling imported by
both `zip.rs` and `rar.rs`). Filename ordering logic (`natural_cmp` and its helpers) was
extracted to `ordering.rs` in #82. The pre-allocation bound (`INITIAL_READ_CAPACITY`,
`initial_capacity`) and the post-materialization rejection (`reject_over_actual`) live here too —
see the two paragraphs below.

Two shared archive-entry helpers also live here so the rule is single-owned rather than
open-coded (and drifting) in each source. `classify_entry(name, is_dir, declared_size, max) ->
EntryClass {Page, Skip, Ignore}` is the page-membership decision both `ZipSource` and `RarSource`
make after `enclosed_name`: directories / non-images / macOS metadata are `Ignore`d as expected
noise, an oversized image is a counted `Skip`, everything else is a `Page` (the membership check
precedes the size check). `cap_or_reject(src, name, max, capacity_hint)` is the read-time streaming
size cap (`take(max + 1)` + length check → `EntryTooLarge`) shared by `FolderSource` and
`ZipSource`. Its `capacity_hint` is a BOUNDED growth hint, never the defense: `ZipSource` derives it
via `initial_capacity(entry.size())`, which clamps the declared size to `INITIAL_READ_CAPACITY`
(1 MiB), and `FolderSource` still passes `0`. A hint must NEVER be derived from an entry's DECLARED
size, because that value is attacker-controlled: a hostile CBZ of a few KB can claim
`MAX_ENTRY_BYTES` per entry, and under rayon prefetch (radius ≤ `MAX_PREFETCH_RADIUS` = 5 ⇒ up to 11
concurrent reads) that turned into GiBs of transient allocation.
`RarSource` cannot stream-cap (`unrar` materializes the whole entry), so its ceiling has THREE
tiers: declared size at open (`classify_entry`), declared `unpacked_size` re-validated at read, and
`reject_over_actual(data, name, max)` — an ACTUAL-length rejection after `cursor.read()` sharing
`cap_or_reject`'s inclusive boundary (`len > max` rejects). The residual is stated, not claimed
away: peak RSS inside `cursor.read()` is still unbounded by the entry's true size, so the third tier
bounds the blast radius — it is **not** parity with Zip/Folder. The format-specific iteration (zip
`by_index` vs rar sequential `read_header`) and the zip-slip guard stay in each source.

### ordering.rs

`ordering.rs`. Shared numeric-aware natural-ordering comparator: `pub(crate) natural_cmp`
plus private helpers `take_digits`/`cmp_numeric`. Numeric-aware so `vol 1 < vol 2 < vol 10`
(digit runs are compared by numeric value, not lexicographically). Extracted from
`page_source::entry_policy` (#82) so the comparator is reachable from both page-source filename
sorting (`FolderSource`/`ZipSource`/`RarSource`) and `Library` book ordering — a private submodule
helper in `entry_policy.rs` could not be reused across the crate.

### ArchiveLoader

`archive_loader.rs` opens directories with `FolderSource` and dispatches archives by
`ArchiveKind`. The case-insensitive extensions `.cbz` and `.zip` select `Zip`, and `.cbr` and
`.rar` select `Rar`. For an unknown extension, a single read of up to six bytes checks the ZIP
signatures and the RAR prefix `Rar!\x1A\x07`. Unrecognized files return `UnsupportedFormat`.
`detect_kind(name, head)` applies the same extension and magic rules to supplied data.

`open_read_at(name, src, policy)` opens ZIP sources through `ReadAt` and returns
`Arc<dyn PageSource>`. It returns `FormatDisabled { format: "rar/cbr" }` for RAR under either
policy setting. `probe_page_count_read_at(name, src, policy)` and `probe_page_count(path)` share
the check that a successfully opened book contains at least one image page. Both return
`NonZeroUsize` or `CoreError::EmptyBook { label }`, and propagate open errors unchanged. The label
in `EmptyBook` or `UnsupportedFormat` is the supplied name or the path's display string. See
[ADR-0009](ADRs/0009-reject-empty-books.md) and [patterns.md](patterns.md).

Without the `rar` feature, both extension and magic-byte detection still recognize RAR, and opening or probing it returns `CoreError::FormatDisabled { format: "rar/cbr" }` regardless of policy.

The path entry points have policy-carrying twins `open_with_policy(path, ArchivePolicy)` and
`probe_page_count_with_policy(path, ArchivePolicy)`. `open` and `probe_page_count` use
`ArchivePolicy::default()`. Safe Mode sets `allow_rar: false`, so either path entry point
returns `CoreError::FormatDisabled` for RAR/CBR. See
[ADR-0011](ADRs/0011-decoder-subprocess-isolation.md) phase 1.

`open_with_policy` also owns the admission gate: a path that is not valid UTF-8 is rejected with
`CoreError::NonUtf8Path { path }` **before any dispatch** (before the `is_dir` branch, the extension
check and the magic sniff), so `probe_page_count_with_policy` inherits it and a path that cannot be
serialized into `library.json` can never become a `Book`. Why the door and not the shelf:
`Book::path` serializes as a JSON string, so ONE non-UTF-8 path made `Library::to_json` — and
therefore EVERY save, for every book — fail wholesale and silently. See
[ADR-0004](ADRs/0004-archive-abstraction-and-extraction.md)'s Amendment.

### image_ops::decode

Returns raw RGBA8 + dimensions. Carries an explicit pixel-count guard `check_pixel_limit`/`MAX_PIXELS`
+ `CoreError::ImageTooLarge` ahead of the `Limits`-bounded decode. A PRIVATE
`decode_dynamic(&[u8]) -> Result<DynamicImage, CoreError>` holds the shared two-layer bomb guard —
header pre-read + `check_pixel_limit` + `Limits`-bounded decode — so BOTH `decode` and
`decode_thumbnail(&[u8], max_side) -> Result<DecodedImage, CoreError>` route through it and the bomb
guard lives in ONE place; a dedicated test proves `decode_thumbnail` inherits the early
`check_pixel_limit` rejection. `downscale_to_thumbnail(&DecodedImage, max_side)` fits an
already-decoded RGBA8 page within the requested square, preserving aspect ratio and leaving
smaller images at their original size. `decode_thumbnail` uses it for RGBA8 input. For other
pixel formats it samples at the same dimensions before converting to RGBA8, preserving the
original sample precision.

See [ADR-0003](ADRs/0003-image-loading-and-caching.md) for image loading decisions.

### thumbnail

`thumbnail.rs`. `generate_one_thumbnail(source: &Arc<dyn PageSource>, max_side, page_index, cache_ctx: Option<PageThumbContext>) -> Result<DecodedImage, CoreError>` —
SYNCHRONOUS single-page thumbnail. With a `PageThumbContext { cache, path }` it reads from / writes to
the on-disk `ThumbnailCache` under `page_cache_key(path, mtime, max_side, page_index)` (a hit skips
the full-page read+decode; the `put` result is ignored); failure is returned as `Err` (never panics).
The desktop backend schedules the work and checks that the session is still open
before delivering the result. The strip uses `DEFAULT_THUMB_MAX_SIDE` (160 px).
The desktop backend also calls the sibling `generate_cover(source, max_side)` to
downsize page zero. It propagates read and decode errors and returns
`IndexOutOfRange { index: 0, len: 0 }` for an empty source.

### thumbnail_cache

`thumbnail_cache.rs`. On-disk QOI cache for page/cover thumbnails under the OS cache dir
(`ProjectDirs("", "", "gashuu").cache_dir()/covers`); `with_dir(PathBuf)` is the tempfile-testable
seam. `put(key, &DecodedImage)` QOI-encodes the RGBA at exact dimensions and writes atomically
(temp-file-then-rename); `get(key) -> Option<DecodedImage>` reads `<dir>/<key>.qoi` and decodes,
returning `None` on any missing/unreadable/corrupt file (a cache miss, never panics). `cache_key(path,
mtime_secs, max_side)` derives a stable 16-hex-char filename via FNV-1a (NOT `DefaultHasher`; see
docs/patterns.md). Headless (no slint/tracing). `DesktopBackend::cover` uses this cache after checking its decoded-cover memory cache.

Issue 143 adds the GC half: `prune(max_bytes) -> PruneReport` sweeps the directory down to
`max_bytes` of `*.qoi` payload in ascending `(mtime, file name)` order, and reclaims stale
`.tmpXXXXXX` crash leftovers (older than an hour) regardless of the cap. `get` refreshes a hit's
mtime (touch-on-get, only after a successful decode), which makes the eviction order near-LRU —
key-orphaned covers (source mtime drifted past `purge_for`) are never read again, age to the
front, and disappear once the cap bites. Best-effort throughout (missing dir → zero report,
failed unlink skipped, foreign files never touched); the caller logs the returned `PruneReport`
(core stays log-free). The cap POLICY is the app layer's: `cover_loader.rs` owns
`CoverCachePolicy::DEFAULT.max_bytes` (256 MiB) and `spawn_cache_prune()`, dispatched once at startup on a
rayon worker right after the initial cover stream.

### cache::ImageCache

LRU of `Arc<DecodedImage>` up to `DEFAULT_CAPACITY`=50 + background ±`DEFAULT_PREFETCH_RADIUS`=3
prefetch in front of any `PageSource`. `get(index)` decodes synchronously on a miss.
`get_cached(index)` returns immediately on a miss and starts neighbour prefetch on a hit.
`dispatch_handle()` returns a cloneable `Send` `CacheDispatch` that shares the LRU via `Arc<Inner>`.
Its `cached(index)` promotes a hit without starting prefetch. `read_raw(index)` returns the
source bytes or the source error. `insert_decoded(index, decoded)` applies the cache's count and
byte limits and returns the shared `Arc`, reusing an existing entry if a concurrent reader
already populated the slot. The synchronous `decode_and_cache` checks the cache before and after
the read, decodes on a miss, and starts neighbour prefetch on success.

`DecodedLru<K>` stores the decoded pages and enforces the count and byte limits.
It supports removing entries by key or predicate and subtracts their decoded
bytes from the total. `ImageCache` keys it by page index.

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
`SpreadMode`/`Auto` — `Auto` is unreachable at the type level in pairing). `SpreadContext` is an
immutable cohesion wrapper bundling `(total, layout, cover)` that delegates to those free fns.
`SpreadNavigation` is the mutable value object above that proved pairing layer: it owns
`(total, spread_mode, cover_mode, viewport_aspect, index)`, resolves `Auto`, and guarantees after
every mutation that `index` is a valid leading for its resolved context. It also owns jump,
next/previous, preview, and resume-persistence derivations. The pure
`scrub_fraction_to_page(fraction, page_count, rtl)` helper lives here as well; direction affects
the raw scrub mapping but remains separate from pairing and page placement.

### viewport

`viewport.rs`. Pure, Slint/tracing-free, STATELESS zoom/pan geometry — fit-scale (`fit_scale`),
pan clamping (`clamp_offset`/`centered_offset`), cursor-anchored zoom (`anchored_zoom`),
`clamp_zoom`, with `ZOOM_MIN`=1.0/`ZOOM_MAX`=8.0, table-tested like `spread`.

### Settings

`settings.rs`. The persistent user-settings domain model and its JSON representation.
`SettingsStore` delegates storage to a `BlobStore` (see below). The view-mode vocabulary it persists
(`reading_direction`/`spread_mode`/`cover_mode`/`fit_mode`/`language`/`key_bindings`) now lives in
`view_modes.rs` (see below); `Settings` is just one consumer. `Settings::view_defaults()`
projects its four global view modes into a `ResolvedView` for per-book override resolution.
`recent_sources` is most-recent-first and capped on both write and load by
`MAX_RECENT_SOURCES = 10`.

**This is the first use of `serde` in core.** The headless boundary still holds (no
slint/tracing). `Settings` keeps only the pure entry points `to_json` / `from_json` / `normalize`
(plus `push_recent` / `cache_config` / `archive_policy` / `view_defaults`); `SettingsStore` owns `load` / `save`.
In `main.rs`, both documents use `load_or_default` for startup recovery. After a failed load, the
helper supplies the current timestamp to the repository's `quarantine` method, records a notice,
and returns the default value. On desktop, a successful quarantine preserves the original file
as `<name>.corrupt-<now_secs>` and the notice includes that name. The startup settings repair
returns immediately when `stored_text` reports an absent blob. Core returns typed `CoreError`:

- `Settings(#[from] serde_json::Error)` and `NoConfigDir`
- `FutureSchema { what, found, supported }` — the stored schema `version` is GREATER than this
  build's; the document is neither deserialized nor saved over (see `persist` below)
- `NonUtf8Path { path }` — raised by `ArchiveLoader::open_with_policy` before any dispatch, so a
  path that cannot be serialized into `library.json` never becomes a `Book`
- `ImageTooLarge`
- `Zip(#[from] ::zip::result::ZipError)`, `EntryTooLarge { name, max }`, `UnsupportedFormat { label }`
- `Rar(#[from] ::unrar::error::UnrarError)` (Display prefix `"rar archive error: "`)
- `EmptyBook { label }` (Display `"no images found in {label}"`) — raised by the `ArchiveLoader` page-count probes on a clean open with zero image pages.

Errors are typed with `thiserror` (`CoreError`, `#[non_exhaustive]`).

See [ADR-0005](ADRs/0005-settings-persistence.md) for the settings persistence decision.

### persist

`persist.rs` contains the parse and migration guard shared by `Settings::from_json` and
`Library::from_json`.

- `parse_versioned_object(json, current, migrate) -> Result<VersionedDocument, serde_json::Error>`
  returns `Ready(serde_json::Value)` (stored version ≤ `current`, already migrated when older) or
  `FromFuture { found }` (stored version > `current`).
- **A stored version GREATER than the binary's is a hard load error** for BOTH `settings.json` and
  `library.json`: this build must never deserialize it (every field is `#[serde(default)]`, so
  unknown fields would be silently dropped) and never save over it (the file would be downgraded
  while keeping its future label, permanently skipping every later migration — each step fires only
  when `stored < target`). Callers map `FromFuture` to `CoreError::FutureSchema { what, found,
  supported }`.
- The non-object-root guard (which would otherwise panic inside `migrate`) and the checked
  `u32::try_from` version resolution still run FIRST, and the `> u32::MAX → treated as 0 → migrate`
  rule is UNCHANGED — a crafted huge version is "unknown", not "from the future".
- The crate-private `quarantine_file(path, now_secs) -> Result<PathBuf, CoreError>` renames the
  file for `FsBlobStore` using the supplied timestamp. The adapter maps a rename's `NotFound`
  error to `Ok(None)` and returns the destination file name on success.
- `Settings::normalize` stamps `SETTINGS_VERSION` — the `version` field is a schema label owned by
  the binary, never echoed back from disk — restoring the symmetry `Library::to_json` already had
  via `LibraryDocument`. `LIBRARY_VERSION` and `SETTINGS_VERSION` both stay `1`.

Headless (`serde_json` + `std::fs` only). See
[ADR-0005](ADRs/0005-settings-persistence.md)'s Amendment.

### library_store / settings_store (the two repositories)

`LibraryStore` and `SettingsStore` each hold an `Arc<dyn BlobStore>` and a key. The port in
`blob_store.rs` reads, atomically replaces, and quarantines byte blobs by flat key. Reads and
quarantines return `Ok(None)` for absent blobs. Both repositories offer `with_blob_store`, which
uses the document's production key, `library.json` or `settings.json`.

- `load` decodes UTF-8 and parses the document. An absent blob yields `Library::new()` or
  `Settings::default()`. Invalid UTF-8 returns an I/O error of kind `InvalidData`.
- `save` passes the existing JSON serialization to the adapter's atomic write. Library version 1
  writes `BookId` as a bare string under each book's `path` key and in `last_opened` when present.
  Desktop loads re-resolve legacy locations and merge aliases before normalization. Web loads
  normalize directly.
- `quarantine(now_secs)` passes the timestamp to the adapter and returns the destination label
  if a blob was moved.
- `SettingsStore::stored_text` supplies the raw text for startup repair.

`FsBlobStore` is available on desktop. Each instance resolves keys relative to its root and
writes through `atomic_write::write_atomic`, which creates missing parent directories. Desktop
startup constructs two instances through the repositories' `default_location()` constructors.
Settings use `FsBlobStore::config_dir()` and the library uses `FsBlobStore::data_dir()` from
`ProjectDirs`. These constructors preserve the existing file locations and return
`NoConfigDir` or `NoDataDir` if directory resolution fails. Desktop `new(path)` constructors
use the parent directory as the adapter root and the file name as the key.
`MemoryBlobStore` uses a mutex-protected map and is available on all targets.
The web implementation is `KvBlobStore<LocalStorageKv>` in the UI crate.

The UI crate holds each store once, behind `Rc`, and threads it as an explicit named parameter to
every function that persists (`main.rs`'s `SettingsStoreHandle` / `LibraryStoreHandle` aliases and
their `save_settings` / `save_library` helpers). The handle is an `Option` because desktop directory
resolution and browser storage detection can fail. The app still runs and each attempted save
reports `NoConfigDir` or `NoDataDir`. The injected
`save` seams (`LeavePointService::persist_with`, `remove_books_with_rollback`, `remove_empty_book_with`,
`OpenBookUseCase`'s two boxed closures, `stage_exit_state`) receive repository saves.
See [ADR-0005](ADRs/0005-settings-persistence.md)'s 2026-08-15 Amendment.

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

`reading_progress.rs` defines the immutable, `Copy` value that each `Book` stores as
`ReadingProgress { last_viewed, total: Option<NonZeroUsize> }`. The resume index is
normally the displayed spread's leading, but the final page INDEX when the
displayed spread contains the final page. Both values reopen onto the same spread because
`ViewerState::jump_to` normalizes them.

Construction clamps `last_viewed` to `total - 1` when the total is known and leaves it verbatim
when the total is unknown (nothing bounds an unknown book). Its derivations are `current()`
(1-based, `last_viewed + 1` saturating, always ≥ 1),
`fraction()` (position-normalized `last_viewed / (total - 1)`, clamped to `0.0..=1.0`), and
`is_finished()` (`last_viewed >= total - 1`). An unknown total and a one-page total both yield
`fraction() == 0.0` and `is_finished() == false`: a one-page book has no meaningful progress bar.
The non-zero total type makes `Some(0)` unrepresentable. A stale constructor input still produces
`fraction() == 1.0` and `is_finished() == true`. `is_at_start()` remains `last_viewed == 0`.

This is the single home of the fraction guard and 1-based offset that BOTH the carousel
(`library_model::carousel_data_for_indices` via `Book::progress()`) and the open-time resume
(`Library::register_opened`) consume. A private `BookRecord` maps the stored progress to the flat
`last_page` and `page_count` fields in `library.json`. The persisted resume remains a
bare page index, so there is no library version bump or eager migration: resumes saved before the
finished-aware write-back reach `100%` after the user revisits the final spread. Headless (no
slint/tracing).

### view_override

`view_override.rs`. Two paired view-preference value objects (headless: no slint/tracing). The
PERSISTED, partial form `ViewOverride { reading_direction, spread_mode, cover_mode, fit_mode }` —
each an `Option<Enum>` where `None` means INHERIT the global default (an active choice, NOT
"unknown"); `Copy`, immutable, `Default`/`none()` == all-`None`, `is_empty()` == all-`None` (the
serde `skip_serializing_if` predicate on `Book::overrides`). The TRANSIENT, total form
`ResolvedView` — every field concrete, never persisted, produced by `Settings::view_defaults()`
or `ViewOverride::resolve(ResolvedView) -> ResolvedView` (the single definition of the per-field
`unwrap_or(global)` fallback). The public inverse
`ViewOverride::differences_from(ResolvedView, ResolvedView) -> ViewOverride` produces the minimal
write-back (`Some` only for fields that differ from global), with the round-trip invariant
`differences_from(view, global).resolve(global) == view`. Re-exported from the crate root
(`pub use view_override::{ResolvedView, ViewOverride}`).
Consumed by the UI's `ViewerState::apply_resolved_view` (+ `ViewportState::set_fit` for the
viewport-owned `fit_mode`). See [ADR-0007](ADRs/0007-per-book-view-overrides.md) for the design
decision and [patterns.md](patterns.md) ("Partial/total override pair") for the value-object flavor.

### Library aggregate

`BookId` identifies a book across the library, viewer, and selection. Its UTF-8 string defines
equality and hashing. Its `Ord` implementation compares path components first to preserve desktop
selection and removal order, then compares the raw strings to distinguish IDs with equal components.
Location handling lives at the following boundaries.

- `canonical_identity` in core resolves a picked desktop path and keeps its original spelling if
  resolution fails or the result is non-UTF-8.
- `Library::recanonicalize_and_merge` interprets stored IDs as locations during desktop loads,
  merges aliases, and remaps `last_opened` to the surviving ID.
- `book_path::identify` builds an ID from a picked location and derives the title from the
  resolved path and the caller's directory flag. `book_path::locate` resolves an ID for file access
  or cache operations. Web IDs are relative OPFS paths resolved by the Worker.

`Library::book(&BookId)` looks up a shelf entry. `add` and `add_many` take IDs and titles supplied by
the caller. `Library::register_opened(id, title, page_count)` adds an entry on its first open and
preserves its stored title on subsequent opens. It applies a known page count and returns
`OpenRegistration { resume: ReadingProgress, count_changed: bool }`. The open use case applies the
book's resolved view before jumping to the returned resume position. It rejects empty sources
before registration (see [ADR-0009](ADRs/0009-reject-empty-books.md)).

`set_resume_page` and `set_page_count` report whether stored progress changed. They delegate to
`ReadingProgress` for clamping. Identity merges construct the survivor's progress from the maximum
resume index and the first known count in shelf order. Deserialization also clamps stale indices.
The version 1 document stores progress as `last_page` and `page_count`, with `0` as the unknown
count. `Book::progress()` provides the in-memory value for registration and carousel projection.

`Library` sorts by natural title order via `ordering::natural_cmp` and breaks title ties with the raw
ID string. Inserts maintain this order and `normalize` restores it on load. Carousel rows and cover
requests share the same projection of the library's order.

`Library` holds `last_opened: Option<BookId>` and exposes it through `last_opened() -> Option<&BookId>`.
`register_opened` sets it, removal clears it when the matching book is deleted, and normalization
clears orphan IDs on load. The usual leave-point saves persist the bookmark alongside reading
progress and overrides.

Each `Book` also carries a `ViewOverride overrides` field (the per-book view-preference overrides;
`#[serde(default, skip_serializing_if = "ViewOverride::is_empty")]`, so an all-`None` override emits
no key and `library.json` stays byte-compatible with files written before this feature — see
[conventions.md](conventions.md), "Add an optional nested config"). The aggregate owns the mutation:
`Library::set_overrides(id, ViewOverride) -> bool` (idempotent-changed bool, same `false`==no-op
convention as `set_last_page`/`set_page_count`) and `Library::overrides_for(id) -> ViewOverride`
(returns `ViewOverride::none()` for an unknown ID, so the caller can `resolve` unconditionally).
There is NO `Book` setter — `Book::overrides()` is read-only.

The bulk-delete rollback primitive (#129) is `Library::restore(books: Vec<Book>, last_opened: Option<BookId>)` —
re-inserts the supplied `Book` clones by ID, restores a valid bookmark, and re-sorts via `book_order`. This is the counterpart to
`remove_many`: a caller that captured clones before removal and then failed to persist can hand
them back to recover the exact pre-removal shelf (byte-identical re-serialization). Avoids the
`add()`-trap (which resets `last_page`/`page_count`) because the full clones carry all existing
field values. Consumed exclusively by `remove_books_with_rollback` in `remove_books.rs`.

### update (pure decision logic)

`update/{version,release,packaging,asset,check,verify,dialog}.rs`. The auto-updater's headless half:
no network, no filesystem, no `slint` — the `gashuu` crate owns HTTP, download and self-replacement,
this tree owns "what should we decide" so every branch is unit-testable on any host. `version.rs`
(`is_update_available`, `should_notify` — the latter also suppresses `Settings.skipped_version`),
`release.rs` (`parse_latest_release` → `ReleaseInfo`/`Asset`), `packaging.rs` (`detect_packaging`
from `current_exe()` + `$APPIMAGE`, `Packaging`, and the `UpdateStrategy {SelfReplace, ManualInstall,
ExternalInstall}` mapping), `asset.rs` (`select_asset`), `check.rs` (`should_check`,
`CHECK_INTERVAL_SECS`), `verify.rs` (`parse_sha256sums`, `sha256_hex`, `is_verified`). All are
re-exported at the crate root. See [ADR-0013](ADRs/0013-auto-update.md).

`update/dialog.rs` holds the two decisions the update dialog delegates:

- `UpdateDialogAction { Accept, Later, Skip, Notes }` + `is_action_allowed(action, in_progress)`.
  **BLOCK semantics**: while a download/verify/install is in flight, `Accept`, `Later` AND `Skip`
  are all refused (dismissal gestures — Esc, Return, backdrop click — alias onto `Later`); `Notes`
  stays allowed because it opens a browser page and neither dismisses the dialog nor touches the
  pipeline. There is deliberately NO cancellation — dismissal is BLOCKED, not aborted.
- `ModalStack { settings, shortcuts, confirm_delete }` + `FocusTarget {ConfirmDialog,
  ShortcutsOverlay, SettingsDialog, Carousel, Pages}` + `focus_target_after_dialog(stack, screen)`,
  which resolves the topmost SURVIVING surface in `ViewerWindow`'s modal paint order: ConfirmDialog
  > ShortcutsOverlay > SettingsDialog > the screen surface (`Carousel` on screen 0, `Pages`
  otherwise). INVARIANT: while ANY modal is still open the result is a modal, never a screen surface
  — the screen `FocusScope`s reject every key under `any-modal-open`, so focusing one under an open
  modal would be a keyboard deadlock.

---

## crates/gashuu-web-decode (wasm32)

`NativePageDecoder` calls `createImageBitmap` on the current `Window` or
`WorkerGlobalScope`. It checks the pixel limit before allocating an `OffscreenCanvas`
and falls back to an HTML canvas on windows without offscreen support. It requests
straight alpha, no color-space conversion, and original pixel orientation. Canvas
readback can round partially transparent RGB values.

## crates/gashuu-worker (wasm32 worker)

The portable library defines the worker protocol, validates book ids, and
dispatches requests using injected decoders. The wasm binary installs the
dedicated worker message handler and posts `Ready` after setup.
Each message carries JSON text in `m` to preserve exact 64-bit ids. Responses
with images carry transferable RGBA8 buffers in `b`, in metadata order.

One request runs at a time. The interactive lane handles opens and spreads
ahead of background thumbnails, covers, page counts, imports, and deletions.
Each lane is FIFO. The loop yields to browser message delivery between
background jobs and between the steps of an import.
Close and cover cancellation run in the message handler and discard outdated
queued work and replies. Each session retains its page source. Pages are
decoded on demand with the browser-native decoder and an image-crate fallback
(PNG and JPEG for books on web). Covers and thumbnails downscale the decoded
image. Both slots of a spread arrive in one reply.

An OPFS book id is its relative path, `books/<uuid>/<sanitised file name>`.
The worker validates it before looking up the file and uses a
`FileSystemSyncAccessHandle` to read at explicit offsets. A weak reader map
lets sessions, cover requests, and page count probes share one handle per
book. Dropping the last reader closes the handle and releases its lock.

An import checks estimated quota, creates a UUID directory and streams the
archive through a synchronous OPFS handle in 8 MiB slices. The worker writes
from a JavaScript typed array, then flushes and probes through the same handle.
It returns the relative ID, display title and nonzero page count. A failed
import closes the handle and removes its directory. `DeleteBook` also removes
the directory recursively after confirming the reader has been released.

## crates/gashuu (Slint presentation layer)

See [ADR-0001](ADRs/0001-gui-framework-slint.md) for the Slint framework decision and
[ADR-0002](ADRs/0002-layered-two-crate-architecture.md) for the two-crate split rationale.

`clock` supplies Unix timestamps and monotonic instants for UI code through `web-time`.

### main.rs

Desktop and wasm `main` both call `run_app` to build the window, wire handlers,
and run Slint's event loop. The desktop-only blocks in `run_app` prune the
cover disk cache, wire drag-and-drop and update handlers, wire persistence on
self-update relaunch, start the update check, restore window geometry, and
persist state after the event loop returns.

### backend (BookBackend port)

`BookBackend` gives the UI an asynchronous book interface over the synchronous
core. Desktop uses rayon and the existing archive, page and thumbnail caches.
`WorkerBackend` handles OPFS books. If the browser lacks OPFS or a Worker
cannot be created, `UnavailableBackend` reports that the browser cannot store
books. Each open starts a session with the caller's cache configuration.
`cached_page` only looks up decoded pages in memory.

A `Done` runs at most once, on the Slint event-loop thread, after the request
returns. At delivery the backend drops callbacks for closed sessions and
cancelled cover generations. Controllers still guard their own UI epochs.

The controller migrations use these methods:

| Controller | Backend methods |
| --- | --- |
| Open and viewer state | `open`, `close` |
| Viewer page loader | `cached_page`, `decode_spread` |
| Thumbnail strip | `thumbnail` |
| Carousel cover loader | `cover` with an optional count callback, `cancel_covers` |
| Bulk add | `probe_page_count` |

#### WorkerBackend (web)

`backend/web.rs` starts one dedicated classic Worker and resolves
`gashuu-worker_loader.js` against the document base URI. `backend/web_client.rs`
owns the protocol state in Rust and buffers requests until the Worker sends
`Ready`. Each reply uses JSON metadata and, when present, transferred RGBA
buffers. The browser adapter copies the buffers into Rust before delivery.

The main thread holds a decoded-page LRU keyed by session and page index.
Its capacity and byte budget come from the latest successful open's
`CacheConfig`. The viewer caps capacity at six pages and sets prefetch to
zero on web. Successful spread slots enter the LRU before delivery.
`cached_page` is a memory lookup. Cover results use the shared 64 MiB
`CoverMemCache`.

Worker replies deliver callbacks from the browser message task on the UI
thread. Cache hits and transport failures defer callbacks through Slint's
event loop. Both paths check session or cover-generation liveness immediately
before invocation. After a Worker failure, pending and future requests receive
`Other { detail: "web worker unavailable" }` and cached pages stay readable.

| Worker error | Backend error |
| --- | --- |
| `EmptyBook` | `EmptyBook` |
| `FormatDisabled` with `rar/cbr` | `FormatDisabled { format: "rar/cbr" }` |
| `FormatDisabled` with another format | `Other` with the format and disabled-support detail |
| `Missing` | `Missing` with the Worker detail |
| `Other` | `Other` with the Worker detail |

### page_decode

`decode_page` returns cached pages immediately. On a miss it reads the encoded page
once and uses core's fallback orchestration. A page cached during either decode takes
precedence. A primary failure triggers one warning per controller when a retry is needed.

The dedicated Worker uses `gashuu_web_decode::NativePageDecoder` as primary and
`ImageCratePageDecoder` as fallback. It downscales page zero for covers and
runs thumbnail requests in the background lane. Desktop uses the image crate. See
[ADR-0014](ADRs/0014-web-decodes-pages-with-the-browser-native-decoder.md).

### web_font

On wasm32, `run_app` schedules a fetch of the Japanese TTF immediately after
creating the window. `web_font` registers it in Slint's shared Fontique
collection as a fallback for Hiragana, Katakana, and Han. Loading runs on
Slint's executor while startup continues. A failure logs a warning.

### BookPicker and BookImporter ports

`BookPicker` returns desktop paths or Send upload handles. Desktop uses rfd
and defers the modal dialog result to the event loop. `web_picker.rs` opens an
attached, hidden multiple-file input during the Add Books click. It uses
`.cbz,.zip` as its accept filter outside iOS-family browsers and checks extensions
after selection on all browsers. The same click requests persistent storage
once per page load.

The picker holds browser File objects in a UI-thread registry. The
`add-picked` callback drains a shared result slot so the UI handler can use its
`Rc` collaborators. Desktop paths go to `BookBackend::probe_page_count`.
The separate `BookImporter` port takes browser upload handles and returns
worker-supplied metadata. `WorkerBackend` takes each file from the registry and
posts it to the worker, retaining it in the outbox if the worker is starting.

At web startup, the app removes demo-era session-only IDs before the first
carousel refresh and saves the cleaned library once. Imported archives live in
OPFS and the library stores their IDs in localStorage. Availability checks for
evicted archives are deferred to #662.

### web_pointer

`web_pointer.rs` provides `apply_pointer_mode`, which `run_app` calls at startup
on wasm32 to set `ViewerWindow.tap-opens-centered` from the `(pointer: coarse)` media query.
The property defaults to false if the query is unavailable. The carousel opens
the focused cover on a single tap when the property is true and selection mode is off.

### web_viewport

On wasm32, `web_viewport` sets the Slint window to the browser's viewport size
in logical pixels at startup and on each browser resize event.

### ViewerState

`ViewerState` holds the active session, canonical book ID, title, listing notices, and display
preferences. Its `SpreadNavigation` computes page pairs, normalizes jumps, and chooses the
resume index. `install_opened` and `close` return the previous session for the caller to release.
Closing resets navigation and metadata and preserves the display modes and next-open cache config.
On web that config limits the cache to six pages and disables neighbour prefetch.

`classify_spread(backend)` looks up each visible index with `cached_page`. A fully cached spread
can render immediately. On a miss, the page controller sends both indices to the backend.
The backend owns decoded pages throughout the session.

The scrubber uses `scrub_fraction_to_page` to translate its fraction to a zero-based page
in either reading direction. `preview_is_double` checks the effective layout without moving
the reading position. Spread and cover changes normalize the position to a valid leading page.
Changing reading direction alters placement while preserving that page.

### ViewportState

`viewport.rs`. UI-layer mutable zoom/pan/fit + viewport size; delegates ALL clamping to core pure
fns. Input entry points: keyboard zoom (`zoom_step` → `zoom_at`), native pinch (`begin_pinch`/`pinch_to`),
drag pan (`begin_pan`/`pan_to`), and scroll pan (`pan_by`).

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

### open_book

`OpenBookUseCase::apply_opened` receives the requested ID and the backend result. It first
persists the outgoing book's position and view overrides. A failed open preserves the current
session. On success it installs the canonical identity, closes the superseded session and
clears the settings dialog's pending inherit intent.

An empty book goes through the shared removal, save and cover-purge transaction. The use case
then closes its session and returns `EmptyBookRejected` before recording a recent or registering
the book (ADR-0009). For a nonempty book it records the requested location when tracking is enabled,
registers the canonical ID and page count, applies the resolved view, jumps to the resume page,
and saves the library. Applying the view before jumping preserves spread alignment (#534).

The use case returns listing and save notices as data. The finalize handler formats them,
refreshes the viewer and carousel, and enters the viewer on success. A missing location keeps
the library visible with the book-named inaccessible notice.

### remove_books

`remove_books.rs`. The "remove the selected books" bulk-delete use case (#129), given its own module
by the #241 split — the destructive parallel to `open_book`. Lean and HEADLESS: every member returns
data and touches no Slint; the UI tail lives in `carousel_refresh::finalize_remove`.

- `RemoveBooksUseCase` — the "remove the selected books" use case. Holds five shared collaborators
  as `Rc<RefCell<…>>` fields: `ViewerState`, `DialogSession`, `Library`, `LibrarySearchState`, and
  `LibrarySelectionState`. `run(&self) -> RemoveOutcome` (no `ui` arg — headless) executes the full
  destructive transaction in the non-negotiable order: snapshot selected IDs → mutate+save with
  rollback (via `remove_books_with_rollback`) → resolve locations with `book_path::locate` and purge
  their covers and page thumbnails → clear the viewer via `ViewerState::close()` and
  its pending intent via `DialogSession::clear_pending_inherit()` when the open book was deleted,
  recording `closed_open_book` in the outcome
  so the Slint side can blank the title bar → recompute the search projection → clear the selection
  (success only; `SaveFailed` preserves it so the user can retry). Returns
  `RemoveOutcome::NoSelection` (empty selection guard), `RemoveOutcome::SaveFailed { error }` (shelf
  rolled back byte-identically via `Library::restore`; selection preserved), or
  `RemoveOutcome::Removed { n, closed_open_book }`. The caller (`handlers/library.rs`'s
  `wire_selection_handlers`) hands the outcome to `carousel_refresh::finalize_remove`, which rebuilds
  the carousel, clamps the focused index, blanks the viewer title (`ui.set_current_book_name("".into())`,
  `carousel_refresh.rs:302`) when `closed_open_book`, and composes the status line.
- `remove_books_with_rollback(library, ids, save)` — the transaction primitive: captures full
  `Book` clones before removal, calls `Library::remove_many`, then calls `save`; on `Err` calls
  `Library::restore(removed_books, prior_last_opened)` and propagates the error. Keeping the full
  books and bookmark makes serialization byte-identical after rollback.
- `removed_contains_open(open_book, removed) -> bool` — pure decision helper: whether the currently
  open book is among the removed IDs.
- `confirm_delete_content(loader, selection, search, library, open_book) -> ConfirmDeleteContent`
  — pure (no I/O, no Slint) builder for the confirm dialog body: title with total count, up to 10
  book titles in ID order, an overflow line when count > 10, a line for books selected outside the
  current search, and a warning
  when the open book is selected. The struct (`title`, `body_lines`, `info`, `warning`) holds
  fully localized strings — each field is resolved via `i18n::dynamic` inside the builder and is
  display-ready.

### view-mode persistence seam (`view_sync.rs`)

`view_sync.rs` owns `ViewModeRoute` and the one-per-app `LeavePointService`. Its `pub(crate)` surface
is `LeavePointService::{new, persist, persist_with}`, `current_runtime_view`,
`apply_global_view_to_runtime`, `apply_runtime_view_to_settings`, and `current_book_name`; its
private members are `stage_view_modes_to_sink`, `position_to_write_back`, `stage_position_write_back`,
`view_override_to_write_back`, and `stage_view_override_write_back`. `persist_with` is the
save-injection seam used by `OpenBookUseCase` and exit staging; `persist` supplies the service's
stored `LibraryStoreHandle` for production handler calls. `apply_runtime_view_to_settings` is
crate-visible because `DialogSession::end` must use the same runtime-to-global definition as the
`DialogClosedOnLibrary` route.

`LeavePointService::persist(route)` is the ONE leave-point persistence chokepoint. The service owns
the state, viewport, dialog-session, settings, library, and store handles once; `persist` delegates
to `persist_with` by injecting `save_library(&library_store, …)`. The transaction stages the
applicable position write-back, delegates view-mode routing
to `stage_view_modes_to_sink`, and saves the library at most once. `stage_view_modes_to_sink`'s
`match route` selects the sink: `DialogClosedOnLibrary` → global reconcile;
`DialogClosedOnViewer` / `LeaveViewer` / `OpenDifferentBook` → per-book write-back; `AppExit` →
per-book write-back first, then a global reconcile when `open_book().is_none()`. `Suspend` uses the
same view-mode sinks when no dialog is open. With an active dialog, suspend writes back the reading
position and preserves the dialog session and its unconfirmed edits
([issue #664](https://github.com/yasuflatland-lf/gashuu/issues/664)).
The global sink runs on Library-dialog close and when no book is open at exit or dialog-free
suspend. A Library-scope `DialogSession::end` also invokes the same helper before restoring its
snapshot. The per-book sink writes an override only when a book is open.
Before a per-book write-back, `stage_view_override_write_back` builds the current `ResolvedView`,
asks `DialogSession::inherit_pending(current)` whether the reset guard still applies, and consults
the shared `Settings` handle to stage only fields that differ from global. The private pure helper
is `view_override_to_write_back(open_book, view, global, inherit_pending)`; pending inherit keeps
priority and emits `ViewOverride::none()` outright. The predicate is derived from runtime equality,
so mode setters do not participate in its maintenance.
`apply_global_view_to_runtime(&settings, &state, &viewport)` mirrors the GLOBAL `Settings` view
modes into the runtime so the Library-screen settings dialog seeds from global (the inverse of
`apply_runtime_view_to_settings`). See [patterns.md](patterns.md), "Per-book view overrides:
write-back-at-leave-point + screen-scoped dialog routing".

### dialog_session

`dialog_session.rs`. `DialogSession` owns the settings-dialog session and records its scope at
open — the seam [ADR-0007](ADRs/0007-per-book-view-overrides.md)'s Decision 4 describes but does
not name.

- `open(scope, state, viewport, settings)` records `DialogScope::Library` or `Viewer`. Library
  scope captures the open book's
  `RuntimeSnapshot { view: ResolvedView, pending_inherit: Option<ResolvedView> }`
  (`None` when no book is open) and THEN global-seeds the runtime via
  `apply_global_view_to_runtime`; Viewer scope leaves the runtime untouched. Persistence routes by
  this recorded scope, while close-time focus restoration routes by the current screen.
- `end(state, viewport, settings)` is idempotent. When the recorded scope is `Library`, it first
  reconciles the LIVE global-edit scratchpad into `Settings` via
  `apply_runtime_view_to_settings`; it then restores any snapshot via `apply_resolved_view`,
  restores `pending_inherit` after the view (verbatim in both directions), and finally clears the
  scope and snapshot. Reconcile-before-restore is load-bearing: reversing those first two steps
  writes the open book's view into the global defaults. Viewer scope and a no-session call leave
  `Settings` untouched. It runs
  from both close-handler branches, exit persistence before `AppExit`, and `on_open_finalize`
  before `apply_probed`. The latter two live inside window-bound code, so each is paired with its
  leave point in one window-free function — `main.rs::stage_exit_state` and
  `handlers/library.rs::end_session_and_apply_probed` — which is what the regression tests call.
- `inherit_pending(current) -> bool` is
  `pending_inherit == Some(current)`, so any runtime mode change disables the guard without a
  setter-side clear; returning to the exact reset view deliberately re-arms it.
- `clear_pending_inherit()` discards the current book's pending intent.
  `OpenBookUseCase::apply_probed` calls it after replacing the open source, and
  `RemoveBooksUseCase::run` calls it when deleting and closing the open book.
- `reset_to_global(&mut self, state, viewport, settings)` applies the globals and THEN records the
  installed runtime view in `pending_inherit`.

Consequence to state plainly: **a global edit made from the Library dialog is committed to
`Settings` but remains invisible to the open book's restored runtime and pending state.** On the
Library screen the live runtime is a global-seeded scratchpad; `end` commits that scratchpad before
restoring the BOOK snapshot. The global edit must not touch the BOOK's pending state — without the
snapshot, the next leave point could write an override pinning the OLD global values onto that book
("Reset to global" silently undone and frozen stale). See
[ADR-0007](ADRs/0007-per-book-view-overrides.md)'s Amendment and [patterns.md](patterns.md),
"Per-book view overrides".

### book_availability

`book_availability.rs` checks existence through `book_path::exists(&book_path::locate(book.id()))`. Unavailable books remain in the library.

### library_model

`library_model.rs`. PURE (Slint-free) `Library` → carousel display-row mapping: a plain
`CarouselData` struct (`title`/`current`/`total`/`progress`/`available`) +
`carousel_data_for_indices(library: &Library, indices: &[usize], is_available: &dyn Fn(&Book) -> bool) -> Vec<CarouselData>`, preserving
the supplied
library-index projection order and skipping out-of-range indices. The indices come from the
natural-order `Library::books()` projection — no independent sort here; see `Library aggregate`
in the core section. The carousel counterpart of `thumbnail_strip`'s row mapping —
keeps the derivation table-testable without a display backend. Each row is built from
`Book::progress()`: 1-based `current = ReadingProgress::current()` (`last_viewed + 1`, saturating);
`progress = ReadingProgress::fraction()` (position-normalized over `total - 1`; an unknown or
one-page total → `0.0`, final page and overshoot clamp to `1.0`);
`total = ReadingProgress::total() -> Option<usize>`. The free derivation no longer lives in `library_model`
— it is centralised in `ReadingProgress` (see core entry below). `available` is derived by an injected predicate, keeping the availability decision outside core. `bookmarked: bool` is a pure derivation computed in
`carousel_data_for_indices`: `Some(book.id()) == library.last_opened()` — true for at most one row
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
`forced_visible_ids` — books added in the current session that stay visible even when the
query would exclude them, until the next query change. Maintains `visible_indices: Vec<usize>` —
the filtered projection of library row indices in natural `Library::books()` order. Recomputes
after every mutation: `set_query(query, &Library)` (clears forced IDs, then recomputes);
`force_visible(ids, &Library)` (dedups against the existing forced set, then recomputes).
`recompute(&Library)` is the entry point for LIBRARY-changed-only cases where neither the query
nor the forced set moved (startup seed, open-time backfill). `visible_indices()` is read-only and
always consistent with the last mutation. The pure `book_matches` helper lives in core `search.rs`;
`matching_indices` lives alongside `LibrarySearchState` and is the fast-path delegate when no IDs
are forced visible.

### LibrarySelectionState

Bulk-delete feature (#126/#128), `library_model.rs` (pub(crate) struct). Owns the active
selection as a `BTreeSet<BookId>` so selected books survive query changes and carousel rebuilds.
`toggle` and `insert` take owned IDs, and `remove` and `contains` take borrowed IDs. The
`selected()` iterator supplies IDs in deterministic order for the confirmation titles and removal
snapshot. `selection_projection.rs` joins the selection with the current search results.

### selection_projection

`selection_projection.rs`. The selection-over-projection JOINS — the four ops that combine the pure
`LibrarySelectionState` ID set with the search projection (`LibrarySearchState::visible_indices()`)
and the `Library`. They are FREE FUNCTIONS (NOT methods on `LibrarySelectionState`), each taking a
`&mut`/`& LibrarySelectionState` as the FIRST argument plus `&LibrarySearchState, &Library`, so the
selection type keeps only its ID set invariant while "operate on just the visible slice" stays a
separate, composable concern:

- `select_visible(&mut selection, search, library)` — select every currently visible book, leaving
  already-selected non-visible books untouched. Drives the Select-all button and Cmd/Ctrl+A.
- `deselect_visible(&mut selection, search, library)` — deselect every currently visible book,
  leaving selected-but-filtered-out books untouched (the orthogonality invariant).
- `all_visible_selected(&selection, search, library) -> bool` — whether every visible book is
  selected (`false` for an empty projection); drives the Select-all toggle direction.
- `visible_selected_count(&selection, search, library) -> usize` — how many selected IDs appear in
  the current visible projection (the toolbar's "N outside search" indicator and `confirm_delete_content`).

All four share a private `visible_paths(search, library)` join and are headlessly unit-tested in-module.

### carousel

`carousel.rs`. UI-thread adapter layer between `library_model` and Slint: `to_carousel_item` (private) maps a `CarouselData` row to a `CarouselItem` (placeholder `slint::Image::default()` cover); `build_carousel_model(library: &Library, indices: &[usize], is_available: &dyn Fn(&Book) -> bool)` (pub(crate)) is a HEADLESS builder — no `ViewerWindow` arg — returning the `Rc<VecModel<CarouselItem>>`; a separate `bind_carousel_model(ui, model)` performs the Slint bind (the build/bind split enables unit tests over visible-index order); `cover_requests(library: &Library, indices: &[usize])` (pub(crate)) derives the per-book `CoverRequest` list with an ID for library updates and a location from `book_path::locate` for source and cache access, re-basing each request's `row` to the enumerated position in the filtered `indices` slice (not the library index), so cover targets stay aligned with the filtered carousel model; `thumb_state_at` (pub(crate)) re-fetches a row's thumbnail state for the scrubber preview.

### carousel refresh / projection (`carousel_refresh.rs`)

`carousel_refresh.rs` (extracted from `main.rs`, mirroring the `view_sync.rs` split). Owns the carousel-refresh/projection cluster: `refresh_library_carousel` (the single chokepoint that rebuilds + binds the filtered carousel model, optionally resets focus, re-applies the ID-keyed selection, and (re)starts focus-prioritized cover loading), the `CarouselRefresh` collaborator bundle it takes (`library` / `library_store` / `covers` / `search` / `selection` / `localizer`, all `pub(crate)`; `library_store` is the `LibraryStoreHandle` the add-path save routes through), the visible-index projection helpers (`visible_index_to_id`, `visible_focus_index_for_id`, `entry_focus_index` (private), `snap_carousel_focus_to_last_opened`, `clamp_focused_index`), and `push_selection_strings` (the selection-toolbar string chokepoint). The `CarouselRefresh` bundle OWNS its `Rc` handles and derives `Clone`: a Slint callback must be `'static`, so each `wire_*` fn whose callbacks rebuild the carousel assembles one bundle from its own parameters and every such callback captures `carousel.clone()`. UI-thread only; driven almost entirely from `handlers/library.rs` and `handlers/settings.rs`, with `go_to_library`/`go_to_viewer` (still in `main.rs`) routing their carousel work through it via the crate-root re-exports.

### page_count_prefetch

`PageCountPrefetch::record_count` displays the resolved total in its carousel row and queues
the book ID and nonzero count for persistence. The backend completion runs on the UI thread
after the cover epoch check. It captures the queue through an `Arc<Mutex<_>>` because backend
callbacks must be `Send`. At the next refresh or shutdown, the controller drains the queue,
applies each count through `Library::set_page_count`, and saves once if any count changed.

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
-> Vec<String>` — they take the language-free content structs from `viewer_state.rs` and
`open_book.rs`
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
the five sub-modules and re-exports all ten `wire_*` fns (plus `start_update_check`) at the
`handlers::` level so `main.rs`
needs no sub-module path. Each `wire_*` fn takes `&ui` and exactly the `Rc` handles its closures
clone — the per-closure `Rc::clone` list IS that handler's dependency list (#151 panel constraint:
no AppState bundle, explicit handle lists only). The five feature files are:

The persistence store handles are part of that explicit list: a `wire_*` fn whose closures save
takes `library_store: &LibraryStoreHandle` and/or `settings_store: &SettingsStoreHandle` as its own
named trailing parameter(s) and clones them per closure like any other handle — never a bundle.
The `CarouselRefresh` bundle (see the carousel refresh section) does not change this: it is never a
`wire_*` parameter. A `wire_*` fn whose callbacks rebuild the carousel still receives the six
handles individually and assembles the bundle in its own body; a callback keeps its own
`Rc::clone` line for any of those handles it also uses outside the bundle.

- **`handlers/library.rs`**: `wire_open_handlers` (add-books,
  add-folder), `wire_carousel_handlers` (carousel search/open/continue-reading/move/back),
  `wire_selection_handlers` (toggle/cover-click/select-all/exit selection; bulk-delete confirm +
  confirm-accepted; empty-book-detected auto-removal). Also constructs the `RemoveBooksUseCase`
  instance inline (the use case's full collaborator list is only available here) and owns the
  `on_empty_book_detected` UI thread handler.
- **`handlers/settings.rs`**: `wire_settings_handlers` (settings dialog open/close, shortcuts
  overlay open/close, reset-overrides),
  `wire_view_mode_handlers` (view-mode setter callbacks — reading direction, spread, cover, fit,
  language, cache/preload/track; the language arm calls `localizer.switch` then `apply`).
- **`handlers/viewer.rs`**: `wire_viewer_input_handlers` (thumbnail click, page-jump, chrome
  reveal, scrubber preview/commit, thumbnail-strip toggle), `wire_viewport_handlers` (viewport
  size + pan/zoom callbacks), `wire_nav_handlers` (keyboard nav hub: page turns, mode toggles,
  GoToLibrary, window resize).
- **`handlers/drag_drop.rs`** is desktop-only (`cfg(not(target_arch = "wasm32"))`).
  `wire_drag_drop_handlers` enables OS file/folder drag-and-drop onto the Library screen.
  It installs the single `on_winit_window_event` filter that toggles the drop-zone
  overlay on hover and debounces a drop's per-file `DroppedFile` burst into one batched
  `AddController::start` (the pure `drop_action_for` winit-event → `DropAction` mapping is the
  testable core). See the dedicated `### handlers/drag_drop` entry below.
- **`handlers/update.rs`** is desktop-only (`cfg(not(target_arch = "wasm32"))`).
  `wire_update_handlers` registers the update dialog's Accept / Later / Skip / Release-notes
  actions behind the in-progress gate and the Settings About-section callbacks.
  `start_update_check` is the startup and on-demand check entry point.
  See the `### update (gashuu)` entry below.

`fn main` = boot (tracing, settings/library load, Slint window, localizer, Rc construction, seed
carousel, prune) + 10 `handlers::wire_*` calls + `handlers::start_update_check` + the crate-local
`wire_relaunch_persistence` bridge + `ui.run()` + `run_exit_persistence` (count persistence,
write-back, view-mode persistence, geometry snapshot, settings save) — the SAME callable the
self-update relaunch invokes via the `persist-before-relaunch` bridge before `relaunch_and_exit`.
`run_exit_persistence` is a thin window-bound shell over `stage_exit_state`, which holds the
window-free prefix (dialog-session reconcile/restore end → `flush_counts` → `AppExit` leave point)
with `settings` threaded as an explicit named parameter and the library save injected through
`LeavePointService::persist_with`, so the ordering is testable without a live window.
All callback closures live in `handlers/`; `main.rs` retains `refresh`, `finalize_open`,
`go_to_library`/`go_to_viewer`, and the add-batch helpers, plus the crate-root re-exports for the
`view_sync.rs` and `carousel_refresh.rs` seams (the carousel-refresh/projection cluster itself now
lives in `carousel_refresh.rs`).

### handlers/drag_drop

`handlers/drag_drop.rs`. OS file/folder drag-and-drop onto the Library screen. Slint exposes no
stable file-drop API, so this bridges the winit backend's raw `WindowEvent`s (behind the
`unstable-winit-030` feature) into the EXISTING bulk-add pipeline — dropped paths are handed straight
to `add_controller::AddController::start`, reusing dedup, the reject rules, the archive policy, and the
add toast (`gashuu-core` untouched; drag-drop is just a new presentation-layer SOURCE of the same
`paths` the Add buttons produce). `drop_action_for(&WindowEvent) -> DropAction` is the pure, testable
mapping (`HoveredFile` → `ShowOverlay`, `HoveredFileCancelled` → `HideOverlay`, `DroppedFile(path)`
→ `Buffer(path)`, everything else → `Ignore`). `wire_drag_drop_handlers(ui, settings, adder)` installs
ONE `on_winit_window_event` filter (the single such slot — `window_state.rs` deliberately installs
none, see below) that toggles the `drag-active` overlay bool and, because winit delivers one
`DroppedFile` per file with NO "drop complete" event, debounces the burst: each drop buffers its path
and re-arms a single-shot `DROP_DEBOUNCE` (50 ms) timer whose tick drains the whole batch into ONE
`start` (so a multi-file drop is one supersede epoch / one progress run / one notice). Add is gated
to `screen == 0`; the filter always returns `EventResult::Propagate`. UI-thread only — no `Send`
state crosses a boundary here.

### update (gashuu)

`update/{mod,net,install}.rs` + `handlers/update.rs` are desktop-only
(`cfg(not(target_arch = "wasm32"))`). This presentation-side half of the auto-updater delegates
every decision to `gashuu_core::update` (see the core `### update` entry).
`mod.rs` holds `CURRENT_VERSION`, the `releases/latest` API + release-page URLs, `user_agent()` and
the `UpdateError {Http, Io, Verify}` glue error.

- **`net.rs`** — `fetch_latest_release_json` / `download_bytes`, blocking, always called from a
  `rayon::spawn` worker. Both go through configured `ureq::Agent`s held in `LazyLock` statics,
  because **ureq v3 ships no default timeouts at all** (every `Timeouts` field defaults to `None`):
  an unconfigured call waits forever on a half-open socket, wedges the dialog in its in-progress
  state, and permanently burns one worker of the rayon pool shared with cover/thumbnail decode. The
  CHECK agent bounds resolve + connect + a global end-to-end budget (a few KB of JSON). The DOWNLOAD
  agent bounds resolve + connect + `recv_response` + `recv_body` and deliberately sets NO global
  budget — a large asset on a slow link is legitimate. ureq 3.3.0 has no per-read/idle/stall
  timeout, so the property bought is "always fails in bounded time", which routes to the existing
  release-page fallback.
- **`install.rs`** — the per-packaging self-replace primitives plus `relaunch_and_exit(exe, persist)`.
  Its ordering is made explicit and unit-testable by `persist_then_spawn(persist, spawn)`: the exit
  persistence runs BEFORE the replacement process is spawned, and only then `exit(0)`. The order is
  load-bearing (the new instance reads `settings.json` / `library.json` at startup), and `exit(0)`
  still happens even when the spawn failed.
- **`handlers/update.rs`** — `wire_update_handlers` + `start_update_check`. Every dialog arm asks
  `gashuu_core::is_action_allowed(action, ui.get_update_in_progress())` first, so the in-progress
  BLOCK is a core decision, not UI-local logic; and every dismissal arm calls the local
  `restore_focus_after_dialog`, which builds a `ModalStack` from the live `show-*` flags and maps
  `gashuu_core::focus_target_after_dialog(stack, ui.get_screen())` onto
  `focus-confirm` / `focus-shortcuts` / `focus-settings` / `focus-carousel` / `focus-pages`.

See [ADR-0013](ADRs/0013-auto-update.md) and its Amendment, and [patterns.md](patterns.md),
"Self-update replaces the running binary DIFFERENTLY per packaging".

### web_lifecycle

`web_lifecycle.rs` registers app-lifetime `pagehide` and hidden `visibilitychange` listeners.
They flush page counts, the suspend leave point, and settings without ending a dialog session.
Both events may flush consecutively; the writes are idempotent.

### web_blob_store

`web_blob_store.rs` adapts browser `localStorage` to `BlobStore`. It stores UTF-8 JSON text as
strings under `gashuu/settings.json` and `gashuu/library.json` to avoid collisions on a shared
origin. Quarantine copies the value to `gashuu/<key>.corrupt-<now_secs>` before removing the
original and returns the unprefixed name for the startup notice. Desktop and web share
`load_persisted` for startup loads, quarantine, and settings repair.

If browser storage is unavailable at startup, the app opens with defaults and an empty library,
logs a warning, and shows `storage (localStorage unavailable)`. Saves report `NoConfigDir` for
settings and `NoDataDir` for the library.

### window_state

`window_state.rs` is desktop-only (`cfg(not(target_arch = "wasm32"))`). It persists and restores
the OS window geometry (size + position) across launches.
Geometry is read ONCE at exit and applied ONCE at startup — never tracked live — so this installs NO
winit event handler and cannot collide with the single `on_winit_window_event` slot drag-drop owns.
Everything is in physical pixels (matching `gashuu_core::WindowGeometry`). `restore_geometry(ui,
settings)` is a no-op when nothing was saved; otherwise it arms a zero-delay single-shot timer
(`arm_apply`) that defers the apply until the lazily-created winit window exists (winit 0.30 creates
it only after `run()` spins), re-arming up to ~30 ticks. `apply_geometry` always applies the clamped
size (`WindowGeometry::floored_size`) and applies the saved position only when it still lands on a
monitor (`is_position_visible`), else centers on the primary monitor (`center_in`).
`capture_geometry(ui, &mut settings)` snapshots the live `size()`/`position()` into
`settings.window` after `run()` returns, or from the relaunch bridge just before the self-update
`exit(0)`. `monitor_rects` enumerates monitor bounds (primary first) via `with_winit_window`,
returning empty on a non-winit build so placement falls back to the OS.

### Slint UI files

`ViewerWindow.web-font-family` drives `default-font-family`. Desktop uses
the empty platform default, and wasm sets it to `Inter` after registering
the Japanese fallback to invalidate cached text shaping.

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
(assets are included transitively through the entry-file import cascade).

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

**`thumbnail_strip.rs`** owns the desktop strip model. On open it creates one placeholder per
page and requests the first 16 pages (or the whole book if shorter) via `BookBackend::thumbnail`.
Scrolling or resizing requests fresh pages in the viewport and its three-page margin on either side.
Each UI-thread completion updates its row to loaded or failed. Closing the session suppresses
pending deliveries, and the strip epoch guards against updates to a replacement model.
Starting without a session clears the strip. The web strip remains empty.

**`cover_loader.rs`** owns the carousel's cover generation. Each refresh persists pending
counts, calls `BookBackend::cancel_covers`, and advances the epoch. `prioritize_by_focus`
sorts requests by distance from the focused row. The controller dispatches 512 px covers
and requests a page count only for rows with an unknown total. Callers release their
library borrow before dispatch so pending counts can be applied. Counts are also flushed
on shutdown and on web suspend.

The desktop backend reads the on-disk cover cache on a worker. On a miss it opens the
archive once for the cover and count. On a hit it queues the cover before opening the
archive for a requested count. Cover size is part of the cache key and is independent
of the strip's 160 px thumbnail size. The web backend serializes page-zero decodes across
refreshes and keeps up to 64 MiB of covers in memory until the app reloads.

Each `ui_done` completion checks the epoch and window before updating its row. An empty book
reported by either callback raises `empty-book-detected` once with its ID. Other cover errors
mark the row failed, while a count error after a hit only produces a warning.

**`add_controller.rs`** probes each picked ID through `BookBackend::probe_page_count` under the
selected archive policy. A path that cannot form an ID produces an unreadable outcome immediately.
Each outcome advances progress in the current generation. The last one invokes `add-finalize`,
whose handler takes the outcomes in input order and applies the batch. Browser imports
share the same progress and finalize path and append any picker notice. Superseding a
batch discards stored imports that never reach the library.

**`open_controller.rs`** passes the raw request ID, policy and next-open cache config to
the backend. A generation ticket accepts one current result. Dropping a superseded result
queues its session for closure on the next dispatch or drain. Taking the current result
transfers session ownership to the open use case.

**`page_loader.rs`** tracks the target spread and reserves missing pages. It passes the session
and both indices to `BookBackend::decode_spread`. Its epoch-checked completion installs both
images together and invokes `spread-anchored` on the UI thread. A failed trailing page degrades
to the leading image. A failed leading page follows the `page-decode-error` path.

**`SettingsDialog.slint`** (issue 102 replaced its std-widgets `ComboBox`/`SpinBox`/`CheckBox` with the token-driven `Segmented`/`Stepper`/`Toggle`/`Dropdown` atoms): modal overlay editing active settings; two-way `current-index <=> in-out-prop` +
`selected`/`edited`/`toggled` callbacks. Since issue 103/104 it is a **content-hug glass panel** (φ relocated into the component proportions; spec 2026-06-04) built from custom `components/` atoms and inheriting the shared `GlassModal` shell for the scrim + panel chrome; its footer "Shortcuts" link opens `ShortcutsOverlay`, and an `in property <int> focus-epoch` (bumped by `ViewerWindow.focus-settings()`) lets the parent re-focus this still-mounted dialog after the overlay closes.
Tab containment is done with two zero-size sentinel `FocusScope`s at the dialog subtree's
document-order edges — `tab-guard-head` declared BEFORE `fs`, `tab-guard-tail` as the LAST child
inside `fs` — each bouncing focus back to `fs` (`changed has-focus => { if (self.has-focus) { fs.focus(); } }`)
the moment window-level tab navigation lands on it, so Tab/Backtab can no longer walk focus onto a
live surface behind the scrim. Enumeration-style rotation is not usable here because the focus chain
is VARIABLE across the drill-in screens 0..5 and ids inside `if` subtrees are unreachable from `fs`;
see [patterns.md](patterns.md), "Modal Tab containment", for the full enumerate-vs-sentinel rule and
the accepted degradation.

**`ShortcutsOverlay.slint`** (issue 104, NEW): a second glass modal listing the keyboard shortcuts read-only, stacked OVER the still-mounted `SettingsDialog` (both `show-settings` and `show-shortcuts` true). Inherits the shared `GlassModal` shell for the scrim + glass panel (`settings-w` width; only `shortcuts-h` is new). Its ancestor `FocusScope` traps every key so focus can't leak to the dialog underneath; closing returns focus to the dialog via the epoch seam. It now also declares its OWN `in property <int> focus-epoch` plus a root-level `changed focus-epoch => { fs.focus(); }`, so a modal dismissed ABOVE it (the update dialog) can hand focus back — the `init` claim still owns the OPEN path, because `changed` does not fire on the initial binding. The keyboard-shortcuts reference text documents the full selection grammar (#129): `x`, `Space`, `Cmd/Ctrl+A`, `Delete`/`Backspace`, `Esc`.

**`components/ConfirmDialog.slint`** (issue 127, NEW; wired in `ViewerWindow` for the bulk-delete path, #129): a GENERIC two-choice confirm/cancel modal — no domain vocabulary. Every string on screen arrives through `in` properties (`title`, `body-lines: [string]`, `info-text`, `warning-text`, `confirm-label`, `cancel-label`) so the same component is reusable across confirm decisions. Inherits the shared `GlassModal` shell (scrim + four-layer fake-glass panel), like `SettingsDialog` / `ShortcutsOverlay`. Mounted in `ViewerWindow` behind `if root.show-confirm-delete` (an `if`-gate so the node is constructed only when needed). Cancel / Esc / backdrop click fire the `cancel` callback (Slint-side: set `show-confirm-delete = false`, restore carousel focus — selection PRESERVED); the confirm `DangerButton` fires the `confirm()` callback (`ConfirmDialog.slint:117`), which `ViewerWindow` forwards as `confirm => { root.confirm-delete-accepted(); }` (ViewerWindow.slint:591), and Rust registers on `ui.on_confirm_delete_accepted` to run `RemoveBooksUseCase` and dismiss the modal. `Enter` is wired to Cancel (the destructive action is never on `Enter`). `ViewerWindow` now also binds `focus-epoch: root.confirm-focus-epoch` (bumped by `ViewerWindow.focus-confirm()`), so the `changed focus-epoch` handler the component had always declared is finally live — it was dead code until the update dialog needed to hand focus back to a surviving confirm dialog.

**`Theme.slint`** (NEW; completed #70): a single Slint `global Theme` that centralises all visual design
tokens — colors, corner radii, spacing, font sizes, component sizes, shadow colors, motion durations, and font weights —
sourced from `/DESIGN.md`. ALL UI components reference `Theme.*`; the previously-inline-hex dialogs (ThumbnailStrip, SettingsDialog) were migrated in #70, and `scripts/check-tokens.sh` is now unconditionally blocking for the whole UI (no allowlist).

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
callbacks, and a FocusScope key-guard. The wiring (dialog lifecycle) was originally registered in `main.rs` and has since
been extracted to `handlers/settings.rs`; the 8 enum↔index helper fns moved to `enum_adapters.rs`; the key-bindings help text is now composed via the localizer and pushed via `ui.set_key_bindings_text(…)` from `handlers/settings.rs`. It carries a two-screen model: `in property <int>
screen` gates the Library `Carousel` (screen 0) vs the Viewer body (screen 1) via
`visible: root.screen == N` (not `if` — see [patterns.md](patterns.md) for the Slint id-scoping
reason); Settings overlays remain viewer-scoped. It mounts the
`Scrubber` as auto-hiding chrome inside the screen-1 viewer,
driven by a `chrome-shown` bool + an idle `Timer`; chrome is revealed on pointer-move (via
`PageView.reveal()`) and scrubber drag — NOT on page turns (arrows / Space / Backspace / tap /
swipe stay quiet so the chrome doesn't flash on every turn). #71 mounted the shared `TitleBar`
component (bound to a new `current-book-name` in-prop — derived by `view_sync.rs`'s
`current_book_name` from `ViewerState::open_title()`, see [patterns.md](patterns.md)),
and set a `min-width`/`min-height` floor on the window. `feat/library-chrome-polish` added
`in property <string> library-count-text` (the pre-composed total book count for the Library
screen's bottom status strip idle state); the strip's `Text` resolves
`text: root.status-text == "" ? root.library-count-text : root.status-text` — transient notices
always win; and forwarded the Carousel's `continue-reading()` callback up to the window level for
Rust handling.

### Library add flow

The macOS Add Books panel can pick archives and folders together. Other desktop
platforms provide a separate Add Folder action. The browser offers one Add
Books action because its picker supports files only.

`AddController` restores input order at finalize. `add_books::apply_outcomes`
deduplicates and inserts successful outcomes, then records their page counts.
Desktop outcomes derive IDs and titles from filesystem paths. Imported
outcomes supply both directly. The handler saves once when books were added,
refreshes the carousel and uses `select_add_notice` to report the result.

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
