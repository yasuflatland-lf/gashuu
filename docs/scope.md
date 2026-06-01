# Scope (shipped baseline and deferred work)

Reference doc migrated from the CLAUDE.md "Scope markers" section.
This file is the authoritative record of what has shipped and what is intentionally out of scope.

## Baseline (shipped)

Shipped across PR1 + PR2 + PR3 + PR4 + PR4a + PR5 + PR6 + PR7 + PR8a + PR8b.

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

### Settings dialog and first-run guide (PR8b)

- A modal dialog (opened from the "Settings…" toolbar button) edits the already-active settings — reading direction / spread / cover / fit (applied immediately) and cache size / preload radius / track-recent (cache/preload apply to newly opened books via `set_cache_config`).
- The keyboard shortcuts are shown read-only (not yet remappable).
- A first-run guide shows once, gated by `Settings::seen_guide`.
- No new deps.

---

## Deferred (intentionally out of scope)

- A genuinely-RAR-compressed test fixture (PR7a / issue #22 — PR7 ships only a store-format fixture, which does not exercise real RAR decompression).
- PR8a thumbnail-strip follow-ups:
  - RTL strip ordering (8a ships ascending order + current-page highlight only).
  - Lazy/on-demand thumbnail generation (8a eagerly generates all).
  - Thumbnail disk cache.
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
- Backdrop-click / Esc dialog dismissal (PR8b dialogs close via their own button only).
- PR5 non-goals: touch/pinch, rotation/minimap/scrollbar, click-to-turn, per-page independent zoom in Double mode, and 60fps is NOT CI-asserted (manual/telemetry only).
