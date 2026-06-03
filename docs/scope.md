# Scope (shipped baseline and deferred work)

Reference doc migrated from the CLAUDE.md "Scope markers" section.
This file is the authoritative record of what has shipped and what is intentionally out of scope.

## Baseline (shipped)

Shipped across PR1 + PR2 + PR3 + PR4 + PR4a + PR5 + PR6 + PR7 + PR8a + PR8b + PR-T + PR-L + PR-V.

### Page sources

- Top-level folder walk (`max_depth(1)`, no recursion).
- CBZ/ZIP archives (PR6, flattened — images at any depth).
- CBR/RAR archives (PR7, extraction only, flattened like ZIP).
- All PNG/JPG/JPEG formats.
- Dispatched by `ArchiveLoader` (extension → magic-byte sniff).

**Intentional deviation from the Issue:** PR6's Issue specified `async_zip` + `tokio` and PR7's plan specified `unrar`, but BOTH use the SYNCHRONOUS crate (`zip` / `unrar`) over the existing rayon pool — the synchronous `read_bytes` trait method plus CPU-bound decode fit rayon naturally, whereas async would force a `block_on` bridge and infect every layer with `tokio`.

The per-entry 500 MB ceiling (`MAX_ENTRY_BYTES` in `naming.rs`) applies to both ZIP and RAR, though RAR's read-time cap is weaker (no streaming `take`; see [docs/patterns.md](patterns.md)).
RAR requires a C++ compiler on every OS (see [docs/toolchain.md](toolchain.md)).

### Cache and prefetch

- LRU page cache (up to `DEFAULT_CAPACITY`=50 decoded images).
- Background ±`DEFAULT_PREFETCH_RADIUS`=3 prefetch.

### Two-page spread, cover mode, and reading direction

- **Two-page spread (Single/Double/Auto)** with active RTL/LTR binding and `cover_mode` (Standalone/Paired).
- Space = next / Backspace = prev (reading order, direction-independent); arrows are direction-aware (LTR → = next, RTL ← = next).
- `Auto` picks single vs double from the window aspect ratio (landscape/square → double, portrait → single) and follows resizes live; composes with RTL/LTR and cover mode.
- Runtime toggles: **D** = spread mode (3-cycle: single → double → auto), **R** = reading direction, **C** = cover mode — each mutates RUNTIME state only (`ViewerState`); `reconcile_settings` mirrors the modes into `Settings` at the next save (PR-D / issue #32), still persisted via save-on-exit (not per-key).

### Zoom, pan, and fit modes

- **Zoom/pan**: wheel = zoom-at-cursor, drag = pan; keys `+`/`-`/`0`/`1`/`f`.
- **Fit modes**: Whole/Width/Actual (`fit_mode` persisted).
- Zoom/pan are session-only (not persisted).
- Explicit image-bomb pixel guard (`check_pixel_limit`/`ImageTooLarge`).

### Settings persistence

- Loaded on startup / saved on exit and on folder-open.
- **`cache_size`, `preload_pages`, `spread_mode` (Single/Double/Auto), `cover_mode`, `reading_direction`, and `fit_mode` are wired to real behavior.**
- `recent_files` recorded only when `track_recent_files` is enabled (off by default for privacy).
- `key_bindings` is persisted but inactive (forward-compat only).

### Parallel thumbnail strip (PR8a)

- rayon-generated thumbnails for all pages, streamed to the UI as each completes.
- Click a thumb or press `T` to jump to any page (via `ViewerState::jump_to`).
- `T` also toggles the strip.
- Failed thumbs render distinctly (red ✕).
- No new deps (reuses `image`/`rayon`).

### Last-read-page resume and Library registration (PR-R)

- Every opened book is registered in the `Library` (`Library::add` on a successful open, with an immediate `save()` mirroring the recents save-on-open).
- The last-read page is written back to `library.json` at every leave point: ↑ to Library, opening another book, and app exit.
- Re-opening a book resumes at its stored last page (via `OpenBookUseCase::run` → `jump_to`). Resume/write-back is observable for books opened through Open Folder / Open Archive.
- **Deferred to PR-C:** populating the carousel from `library.books()` and cover/visual display. The `on_carousel_open` path is wired (resolves carousel index → book path → `OpenBookUseCase::run`) but inert until the carousel is actually populated. (A seam-only version would be inert because no book is added to the Library — that gap, surfaced in review, is why `Library::add`-on-open shipped here.)

### Settings dialog and first-run guide (PR8b)

- A modal dialog (opened from the "Settings…" toolbar button) edits the already-active settings — reading direction / spread / cover / fit (applied immediately) and cache size / preload radius / track-recent (cache/preload apply to newly opened books via `set_cache_config`).
- The keyboard shortcuts are shown read-only (not yet remappable).
- A first-run guide shows once, gated by `Settings::seen_guide`.
- No new deps.

### Library cover-flow carousel rendering (PR-C)

- The Library screen (the PR-0b two-screen shell) now RENDERS the cover-flow carousel from the `Library` model: focused cover with accent ring, scaled/dimmed neighbors, per-cover + focused-meta reading-progress bars, a grayed broken-cover placeholder for unavailable books, and the 0-book empty-state CTA. Built from the pure `library_model::carousel_data` mapping via the `carousel::to_carousel_item` UI-thread adapter (covers via `slint::Image::default()` for now). No new deps.
- **Covers start as placeholders** — PR-V (below) streams the real cover images in (into the same `VecModel<CarouselItem>`, using the PR8a `invoke_from_event_loop` pattern).
- Per-book page `total` was a placeholder `0` in PR-C; **PR-La now persists it** (`Book::page_count`, back-filled and saved on open) and the carousel shows the real `total` + `current / total` progress fraction once a book has been opened. See "Per-book page totals, fallible save, and load-failure notice (PR-La)" below. (Later: `CoverController` ALSO prefetches the count for never-opened books in the background, so the carousel no longer shows `1 / 0` before first open — see "Cover sharpness, page-count prefetch, cover-flow z-order" below.)
- The empty-state CTA is wired to the file/folder picker by PR-L (see below).

### Thumbnail disk cache (PR-T)

`ThumbnailCache` (gashuu-core) persists thumbnails/covers as PNG files under the OS cache directory, keyed by a version-stable FNV-1a hash of (path, mtime, max-side). `put` writes atomically (temp-file-then-rename); `get` returns `None` on miss/corrupt. This is the storage primitive; the cover carousel that consumes it is PR-V (below). Concurrent same-key write safety is deferred (see docs/patterns.md) and is not exercised by PR-V (one distinct key per book).

### Library carousel covers (PR-V)

- The Library carousel now shows each book's REAL cover (the thumbnail of its page 0), streamed in row-by-row rather than the PR-C placeholders. Core gained `generate_cover(source, max_side) -> Result<DecodedImage, CoreError>` (page-0 thumbnail; errors on a 0-page source); the UI's new `cover_loader.rs` (`CoverController`, a twin of the `thumbnail_strip.rs` controller) tries the `ThumbnailCache` first, else fires a `rayon::spawn` worker that opens via `ArchiveLoader`, calls `generate_cover`, caches the result, and marshals it into the shared `VecModel<CarouselItem>` via `invoke_from_event_loop` — under the same epoch + cancel double-guard as the thumbnail strip (see docs/patterns.md).
- `rayon` became a direct dep of the `gashuu` UI crate (no new lockfile entry — see docs/toolchain.md).
- No new lockfile dependencies. Core `generate_cover` is unit-tested (page-0 selection, empty-source error, decode-error propagation); the UI streaming path in `cover_loader.rs` is coverage-exempt like the thumbnail strip (see docs/quality-gates.md).

### Multi-file loading via picker (PR-L)

- **Add files** (`rfd` `pick_files`, filtered cbz/zip/cbr/rar) and **Add folder** (`pick_folder`, folder-as-one-book) toolbar buttons on the Library screen, plus an interactive empty-state CTA that fires the file picker.
- Adds route through `add_paths` (dedup via `Library::add`, skipping books already present and duplicates within the batch), then persist the library, rebuild the carousel model (`build_carousel_model`), and surface the outcome on a Library-screen status line.
- Library is loaded at startup (corrupt/unreadable → empty library, same UI-layer recovery policy as `Settings`) and the carousel is seeded from it on boot.
- No new deps (reuses `rfd`).

### Per-book page totals, fallible save, and load-failure notice (PR-La)

- **Per-book page totals are modeled + persisted.** `Book::page_count` (a `#[serde(default)]` field, no `LIBRARY_VERSION` bump) is back-filled from the opened source's page count and saved to `library.json` on open, so the count survives relaunch. The cover-flow carousel (`carousel_data`) now shows the REAL `total` / `current` / progress fraction for any opened book (`0` = never opened until prefetched — see below). Same-session visibility comes from a carousel rebuild on the open path when the count was just back-filled. Cover images remain placeholders until PR-V.
- **Save is fallible end-to-end** — `Library::to_json -> Result` (symmetric with `from_json`); `save`/`save_to` propagate via `?`. No serialize step is silently swallowed, so a save can no longer write a truncated file while the UI reports success.
- **Startup load failures surface on the home screen.** A genuine `Library`/`Settings` load failure (corrupt data / I/O / `NoDataDir`; missing files still return `Ok(default)`) is collected and shown on the Library status line after the initial refresh. Closes the PR-L gap where library load failure was tracing-only.
- No new deps.

### Cover sharpness, page-count prefetch, cover-flow z-order

- **Sharper covers.** `cover_loader::COVER_MAX_SIDE` rises from 160 → **512 px**, decoupled from the page strip's `DEFAULT_THUMB_MAX_SIDE` (160). A focused cover slot is up to 240×336 logical px (≈480×672 physical at 2× Retina), so the old 160 px buffer upscaled ~4× and looked blurry. `max_side` is part of the `cache_key`, so the bump transparently invalidates and regenerates every stale 160 px cover on the next run. The strip path is unchanged.
- **No more `1 / 0` before first open.** `CoverController` prefetches the REAL page count for never-opened books (`Book::page_count_opt() == None`, flagged `needs_count` in `cover_requests`). The cover-MISS worker reuses its archive open to read `list_pages().len()`; a cover-cache HIT spawns a count-only open (`spawn_count_only`). The count is streamed to the row's `total` immediately (`marshal_total`) and queued in `pending_counts` for UI-thread persistence — applied via `Library::set_page_count` + `save` at the next `start` and at shutdown (`flush_counts`), so it survives a relaunch (no re-open needed next time). A worker can't touch the `!Send` `Rc<RefCell<Library>>`, hence the queue + UI-thread drain.
- **Focused cover always on top.** The cover-flow now renders via a file-private `CoverCard` sub-component in TWO `for` passes (neighbors, then the focused card declared after → drawn on top), since Slint 1.x has no per-`Repeater`-item z. Both passes bind identical geometry so the Left/Right slide still animates continuously. Fixes the neighbor card overlapping the centered cover.
- No new deps. The UI streaming/controller path stays coverage-exempt (same as the thumbnail strip); the pure `cover_requests` `needs_count` derivation is unit-tested.

### Library search (#88)

- Live filter in the glass-pill NavBar by title and filesystem path; 120ms trailing debounce keeps the carousel responsive while typing.
- Freshly-added books remain visible until the query changes (the library model appends before filtering, so newly added books are not hidden mid-session).
- Filtered carousel rows map through a visible-index projection so open / move / add operations resolve the correct book even when only a subset is shown.
- The Settings button in the same nav bar opens the existing `SettingsDialog` — no new entry points.
- No new dependencies.

### Per-book view overrides with global fallback

- Reading direction, spread mode, cover mode, and fit mode are now **per-book overrides** backed by a `#[serde(default)]`-gated `ViewOverride` on `Book` (backward-compatible; missing field → `None` → inherit global `Settings`). Screen decides scope: the Library settings dialog edits the global `Settings` defaults; the Viewer settings dialog and the in-viewer D/R/C/fit toggles edit the current book's override (written back at every leave point). The Viewer settings dialog also exposes a "Reset to global" button.

### Release builds — macOS + Windows executables (release workflow)

- `.github/workflows/release.yml` builds a macOS universal `.app` (lipo'd arm64+x86_64, zipped via `ditto`) and a Windows `gashuu.exe` (icon embedded via `winresource`, zipped), and attaches both to the GitHub Release for a `v*` tag (or a `workflow_dispatch` tag input). A `preflight` job gates on tag == `crates/gashuu/Cargo.toml` version before building. See [docs/toolchain.md](toolchain.md) "Release builds".
- Unsigned (MVP); `release.yml` documents the macOS notarization / Windows Authenticode seams for later.
- Windows `.ico` embedding (previously deferred) now ships via a `cfg(windows)`-gated `winresource` build-dependency + `build.rs`. The only new dependency is `winresource` (Windows-only build-dep); no runtime deps.

### UI language switch — English / 日本語 (i18n)

- The UI is bilingual with **immediate, no-restart switching** from a new "General → Language" pull-down in the settings dialog (the Apple-HIG-style `Dropdown` atom — the repo's first `PopupWindow`). The preference persists as `Settings.language` (`"en"`/`"ja"` serde tags doubling as the Slint locale names; `#[serde(default)]`; global-only — never per-book).
- `.slint` strings are `@tr()`-wrapped and translated via a bundled gettext catalog (`crates/gashuu/translations/ja/LC_MESSAGES/gashuu.po`, compiled in by `build.rs` — see docs/patterns.md for the msgctxt gotcha). Rust-composed strings (status line, open/save notices, decode errors) go through `src/messages.rs` (exhaustive per-language `match`). `KEY_BINDINGS_HELP` stays English until the ShortcutsOverlay renders it on screen.

---

## Deferred (intentionally out of scope)

- A genuinely-RAR-compressed test fixture (PR7a / issue #22 — PR7 ships only a store-format fixture, which does not exercise real RAR decompression).
- PR8a thumbnail-strip follow-ups:
  - RTL strip ordering (8a ships ascending order + current-page highlight only).
  - Lazy/on-demand thumbnail generation (8a eagerly generates all).
  - Virtual scroll for huge archives.
- Nested archives.
- `ComicInfo.xml` metadata.
- Password-protected ZIP/RAR.
- Per-entry offset/cursor-cache optimization (avoid reopening the central directory / re-walking from the front on every `read_bytes`).
- Multi-volume/split RAR.
- Solid-RAR skip-speedup.
- User-remappable keys (`key_bindings` stays persisted-but-inactive; the PR8b dialog shows a read-only reference).
- Immediate runtime rebuild of the CURRENT book's cache (`ViewerState::rebuild_cache` — PR8b's `set_cache_config` only affects newly opened books).
- `recent_files`-management / theme settings UI.
- All three PR-L follow-ups SHIPPED in PR-La (see "Per-book page totals, fallible save, and load-failure notice" above): on-screen library-load-failure notice, fallible `to_json` (no silent serialize-error discard on save), and real carousel `total`/`progress` via persisted page counts. Covers now stream in via PR-V (see "Library carousel covers" above).
- Backdrop-click / Esc dialog dismissal (PR8b dialogs close via their own button only).
- PR5 non-goals: touch/pinch, rotation/minimap/scrollbar, click-to-turn, per-page independent zoom in Double mode, and 60fps is NOT CI-asserted (manual/telemetry only).
- Linux release artifacts and a `.desktop` entry (macOS + Windows ship via `release.yml`; Linux's Slint system-library deps make a portable artifact heavier — deferred).
- Code signing / notarization for release binaries (macOS Developer ID + notarytool, Windows Authenticode) — `release.yml` ships unsigned with documented `SIGNING SEAM` insertion points.
- Auto-creating the GitHub Release (the release must pre-exist; `release.yml` only uploads assets to it).
