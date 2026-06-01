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

PR6. Shared `natural_cmp`/`has_image_ext`/`IMAGE_EXTS` (`pub(crate)`) EXTRACTED from `folder.rs`
so `FolderSource`/`ZipSource`/`RarSource` sort/recognize identically. PR7 added `enclosed_name`
(`pub(crate)` traversal/zip-slip guard mirroring `zip`'s, rejecting absolute / root-or-prefix /
any `..`, so RAR entries get the same protection ZIP gets) and MOVED `MAX_ENTRY_BYTES` here from
`zip.rs` (now a neutral shared `pub(crate)` 500 MB archive-entry ceiling imported by BOTH
`zip.rs` and `rar.rs`).

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
owns the fire-and-forget spawn" philosophy as `ImageCache`.

### cache::ImageCache

LRU of `Arc<DecodedImage>` up to `DEFAULT_CAPACITY`=50 + background ±`DEFAULT_PREFETCH_RADIUS`=3
prefetch in front of any `PageSource`.

See [ADR-0003](ADRs/0003-image-loading-and-caching.md) for the LRU/prefetch decision.

### spread

`spread.rs`. Pure, Slint/tracing-free, reading-direction-agnostic page-pairing
(`spread_at`/`next_leading`/`prev_leading`/`normalize_leading` over
`Spread {leading, trailing: Option<usize>}`); pairing functions take `SpreadLayout` (never
`SpreadMode`/`Auto` — `Auto` is unreachable at the type level in pairing).

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

---

## crates/gashuu (Slint presentation layer)

See [ADR-0001](ADRs/0001-gui-framework-slint.md) for the Slint framework decision and
[ADR-0002](ADRs/0002-layered-two-crate-architecture.md) for the two-crate split rationale.

### ViewerState

Navigation backed by `ImageCache`; drives a two-page spread via
`current_spread() -> Option<Result<SpreadImages, CoreError>>`, with `apply` moving in spread units.
PR6 `open_path(path)` dispatches via `ArchiveLoader` + skipped warn + a `last_open_skipped()` getter,
and `open_folder` now delegates to it. PR8a added `jump_to(page) -> bool` — routes through
`normalize_leading` so `index` stays a valid spread leading, clamps out-of-range, guards
`page_count==0` to avoid underflow, and returns whether it moved, mirroring `set_viewport_size`'s
"did it change → caller refreshes" convention — and `current_source() -> Option<Arc<dyn PageSource>>`
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

### ViewportState

`viewport.rs`. UI-layer mutable zoom/pan/fit + viewport size; delegates ALL clamping to core pure
fns.

### keymap

Direction-aware key token → `KeyCommand` — page turns, D/R/C mode toggles, plus
direction-independent zoom/fit commands and PR8a's `ToggleThumbnails` on `"t"`,
direction-INDEPENDENT like the zoom/fit keys.

### Slint UI files

**`ThumbnailStrip.slint`** (PR8a, NEW): horizontal `Flickable` + `HorizontalLayout` + `for` over a
`VecModel` — the FIRST `VecModel`/`Repeater` use in the codebase since `ListView` is
vertical-only — over `struct ThumbnailItem { image, page, loaded, failed }`. `main.rs` drives it
with a background `std::thread::spawn(generate_thumbnails(...))`, an `Rc<VecModel<ThumbnailItem>>`,
an epoch+cancel double-guard, and a `slint::invoke_from_event_loop` marshal.

**`SettingsDialog.slint`** (PR8b, NEW): modal overlay editing active settings via std-widgets
`ComboBox`/`SpinBox`/`CheckBox`; two-way `current-index <=> in-out-prop` +
`selected`/`edited`/`toggled` callbacks.

**`FirstRunGuide.slint`** (PR8b, NEW): dismissable once-only overlay; a local `GuideLine`
component dedupes the key-reference rows.

**`ViewerWindow.slint`**: extended in PR8b with the two `if root.show-X : Component` overlays
(last children = front), a "Settings…" toolbar button, the in/in-out properties + setter
callbacks, and a FocusScope key-guard. `main.rs` gained the dialog/guide wiring + 8 enum↔index
helper fns + `KEY_BINDINGS_HELP`.

### rfd file/folder picker

PR6 `on_open_archive` → `rfd` `pick_file` filtered to cbz/zip. PR7 extended the filter to
cbz/zip/cbr/rar — the ONLY UI change in PR7 since `open_path` already dispatched via
`ArchiveLoader`. "Open Archive" button lives in `ViewerWindow.slint`.

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
