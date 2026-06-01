# Scope (shipped baseline and deferred work)

Reference doc migrated from the CLAUDE.md "Scope markers" section.
This file is the authoritative record of what has shipped and what is intentionally out of scope.

## Baseline (shipped)

Shipped across PR1 + PR2 + PR3 + PR4 + PR4a + PR5 + PR6 + PR7 + PR8a + PR8b + PR-T + PR-L.

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
- Re-opening a book resumes at its stored last page (via `open_and_present` → `jump_to`). Resume/write-back is observable for books opened through Open Folder / Open Archive.
- **Deferred to PR-C:** populating the carousel from `library.books()` and cover/visual display. The `on_carousel_open` path is wired (resolves carousel index → book path → `open_and_present`) but inert until the carousel is actually populated. (A seam-only version would be inert because no book is added to the Library — that gap, surfaced in review, is why `Library::add`-on-open shipped here.)

### Settings dialog and first-run guide (PR8b)

- A modal dialog (opened from the "Settings…" toolbar button) edits the already-active settings — reading direction / spread / cover / fit (applied immediately) and cache size / preload radius / track-recent (cache/preload apply to newly opened books via `set_cache_config`).
- The keyboard shortcuts are shown read-only (not yet remappable).
- A first-run guide shows once, gated by `Settings::seen_guide`.
- No new deps.

### Library cover-flow carousel rendering (PR-C)

- The Library screen (the PR-0b two-screen shell) now RENDERS the cover-flow carousel from the `Library` model: focused cover with accent ring, scaled/dimmed neighbors, per-cover + focused-meta reading-progress bars, a grayed broken-cover placeholder for unavailable books, and the 0-book empty-state CTA. Built from the pure `library_model::carousel_data` mapping via the `to_carousel_item` UI-thread adapter (covers via `slint::Image::default()` for now). No new deps.
- **Covers are PLACEHOLDERS** — real cover images stream in via PR-V (into the same `VecModel<CarouselItem>`, using the PR8a `invoke_from_event_loop` pattern).
- Per-book page `total` was a placeholder `0` in PR-C; **PR-La now persists it** (`Book::page_count`, back-filled and saved on open) and the carousel shows the real `total` + `current / total` progress fraction once a book has been opened. See "Per-book page totals, fallible save, and load-failure notice (PR-La)" below.
- The empty-state CTA is wired to the file/folder picker by PR-L (see below).

### Thumbnail disk cache (PR-T)

`ThumbnailCache` (gashuu-core) persists thumbnails/covers as PNG files under the OS cache directory, keyed by a version-stable FNV-1a hash of (path, mtime, max-side). `put` writes atomically (temp-file-then-rename); `get` returns `None` on miss/corrupt. This is the storage primitive; the cover carousel that consumes it is PR-V. Concurrent same-key write safety is deferred (see docs/patterns.md).

### Multi-file loading via picker (PR-L)

- **Add files** (`rfd` `pick_files`, filtered cbz/zip/cbr/rar) and **Add folder** (`pick_folder`, folder-as-one-book) toolbar buttons on the Library screen, plus an interactive empty-state CTA that fires the file picker.
- Adds route through `add_paths` (dedup via `Library::add`, skipping books already present and duplicates within the batch), then persist the library, rebuild the carousel model (`build_carousel_model`), and surface the outcome on a Library-screen status line.
- Library is loaded at startup (corrupt/unreadable → empty library, same UI-layer recovery policy as `Settings`) and the carousel is seeded from it on boot.
- No new deps (reuses `rfd`).

### Per-book page totals, fallible save, and load-failure notice (PR-La)

- **Per-book page totals are modeled + persisted.** `Book::page_count` (a `#[serde(default)]` field, no `LIBRARY_VERSION` bump) is back-filled from the opened source's page count and saved to `library.json` on open, so the count survives relaunch. The cover-flow carousel (`carousel_data`) now shows the REAL `total` / `current` / progress fraction for any opened book (`0` = never opened, reads as unread via the existing guard). Same-session visibility comes from a carousel rebuild on the open path when the count was just back-filled. Cover images remain placeholders until PR-V.
- **Save is fallible end-to-end** — `Library::to_json -> Result` (symmetric with `from_json`); `save`/`save_to` propagate via `?`. No serialize step is silently swallowed, so a save can no longer write a truncated file while the UI reports success.
- **Startup load failures surface on the home screen.** A genuine `Library`/`Settings` load failure (corrupt data / I/O / `NoDataDir`; missing files still return `Ok(default)`) is collected and shown on the Library status line after the initial refresh. Closes the PR-L gap where library load failure was tracing-only.
- No new deps.

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
- All three PR-L follow-ups SHIPPED in PR-La (see "Per-book page totals, fallible save, and load-failure notice" above): on-screen library-load-failure notice, fallible `to_json` (no silent serialize-error discard on save), and real carousel `total`/`progress` via persisted page counts. Covers still stream in via PR-V.
- Backdrop-click / Esc dialog dismissal (PR8b dialogs close via their own button only).
- PR5 non-goals: touch/pinch, rotation/minimap/scrollbar, click-to-turn, per-page independent zoom in Double mode, and 60fps is NOT CI-asserted (manual/telemetry only).
