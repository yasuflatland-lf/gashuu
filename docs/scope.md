# Scope (shipped baseline and deferred work)

Reference doc migrated from the CLAUDE.md "Scope markers" section.
This file is the authoritative record of what has shipped and what is intentionally out of scope.

## Baseline (shipped)

### Page sources

- Top-level folder walk (`max_depth(1)`, no recursion).
- CBZ/ZIP archives (flattened — images at any depth).
- CBR/RAR archives (extraction only, flattened like ZIP).
- All PNG/JPG/JPEG/AVIF formats (AVIF decode via dav1d — see [docs/toolchain.md](toolchain.md)).
- Dispatched by `ArchiveLoader` (extension → magic-byte sniff).

**Intentional deviation from the Issue:** the ZIP Issue specified `async_zip` + `tokio` and the RAR plan specified `unrar`, but BOTH use the SYNCHRONOUS crate (`zip` / `unrar`) over the existing rayon pool — the synchronous `read_bytes` trait method plus CPU-bound decode fit rayon naturally, whereas async would force a `block_on` bridge and infect every layer with `tokio`.

The per-entry 500 MB ceiling (`MAX_ENTRY_BYTES` in `naming.rs`) applies to both ZIP and RAR, though RAR's read-time cap is weaker (no streaming `take`; see [docs/patterns.md](patterns.md)).
RAR requires a C++ compiler on every OS (see [docs/toolchain.md](toolchain.md)).

### Cache and prefetch

- LRU page cache (up to `DEFAULT_CAPACITY`=50 decoded images).
- Background ±`DEFAULT_PREFETCH_RADIUS`=3 prefetch.

### Two-page spread, cover mode, and reading direction

- **Two-page spread (Single/Double/Auto)** with active RTL/LTR binding and `cover_mode` (Standalone/Paired).
- Space = next / Backspace = prev (reading order, direction-independent); arrows are direction-aware (LTR → = next, RTL ← = next).
- **Pointer / gesture page-turn**: a single click/tap (left half → `nav("left")`, right half → `nav("right")`) and a two-finger trackpad horizontal swipe (≥60 px, with an idle-gap guard against momentum-scroll re-fire) both route through the SAME `nav()` seam as the arrow keys, so reading direction is resolved once (RTL: left = next). The horizontal swipe turns pages whenever the page fits HORIZONTALLY (dominant-axis gate + `content-w <= width`), so a tall Width-fit page still turns on a flick; only when the content overflows horizontally (zoomed in / wider than the viewport) does two-finger scroll pan instead, and vertical scroll always pans a fit page that overflows vertically. Both page-turn paths are suppressed under any open modal.
- `Auto` picks single vs double from the window aspect ratio (landscape/square → single, portrait → double) and follows resizes live; composes with RTL/LTR and cover mode.
- Runtime toggles: **D** = spread mode (3-cycle: single → double → auto), **R** = reading direction, **C** = cover mode — each mutates RUNTIME state only (`ViewerState`); `reconcile_settings` mirrors the modes into `Settings` at the next save (issue #32), still persisted via save-on-exit (not per-key).

### Zoom, pan, and fit modes

- **Zoom/pan**: backed by the pure core `viewport.rs` (`fit_scale` / `clamp_zoom` [`ZOOM_MIN`=1.0, `ZOOM_MAX`=8.0] / `anchored_zoom` / `clamp_offset`) and the presentation-layer `ViewportState` (`src/viewport.rs`: `zoom_at` / `zoom_step` / `pinch_to` / `begin_pinch` / `pan_to` / `pan_by` / `begin_pan` / `set_fit` / `cycle_fit`), which holds the live zoom/pan/fit. Input: native trackpad/touch pinch (`ScaleRotateGestureHandler` → `ViewportState::pinch_to`, focal-anchored) and keys `+`/`-` (`zoom_step`, center-anchored) zoom; two-finger scroll pans while zoomed in (`scroll-pan` → `pan_by`) and turns pages at fit; drag = pan (`pan_to`); keys `0`/`1`/`f` reset/fit. Two-finger scroll no longer zooms.
- **Fit modes**: Whole/Width/Actual (`fit_mode` persisted).
- Zoom/pan are session-only (not persisted).
- Explicit image-bomb pixel guard (`check_pixel_limit`/`ImageTooLarge`).

### Settings persistence

- Loaded on startup / saved on exit and on folder-open.
- **`cache_size`, `preload_pages`, `spread_mode` (Single/Double/Auto), `cover_mode`, `reading_direction`, and `fit_mode` are wired to real behavior.**
- `recent_files` recorded only when `track_recent_files` is enabled (off by default for privacy).
- `key_bindings` is persisted but inactive (forward-compat only).

### Parallel thumbnail strip

- rayon-generated thumbnails, streamed to the UI as each completes. Generation is **lazy** (later evolution, not the original eager slice): `start` paints `N` `Loading` placeholders but decodes only the first `INITIAL_VISIBLE_PAGES` (16); the rest backfill on demand as the strip scrolls/resizes (`visible-range-changed`, `VISIBLE_MARGIN`=3), each batch streamed in. Reuses the per-page disk cache (`PageThumbCache`) across opens; an epoch + cancel double-guard stops a superseded book's batches.
- Click a thumb or press `T` to jump to any page (via `ViewerState::jump_to`).
- `T` also toggles the strip.
- Failed thumbs render distinctly (red ✕).
- No new deps (reuses `image`/`rayon`).

### Last-read-page resume and Library registration

- Every opened book is registered in the `Library` (`Library::add` on a successful open, with an immediate `save()` mirroring the recents save-on-open).
- The last-read page is written back to `library.json` at every leave point: ↑ to Library, opening another book, and app exit.
- Re-opening a book resumes at its stored last page (via `OpenBookUseCase::run` → `jump_to`). Resume/write-back is observable for books opened through Open Folder / Open Archive.
- The `on_carousel_open` path is wired (resolves carousel index → book path → `OpenBookUseCase::run`). It was originally inert until the carousel was populated (covered by "Library cover-flow carousel rendering" below). (A seam-only version would have been inert because no book is added to the Library — that gap, surfaced in review, is why `Library::add`-on-open shipped here.)

### Settings dialog and first-run guide

- A modal dialog (opened from the "Settings…" toolbar button) edits the already-active settings — reading direction / spread / cover / fit (applied immediately) and cache size / preload radius / track-recent (cache/preload apply to newly opened books via `set_cache_config`).
- The keyboard shortcuts are shown read-only (not yet remappable). Since issue 104 they live in a separate `ShortcutsOverlay` modal opened from the dialog's footer "Shortcuts" link, not inline in the dialog.
- A first-run guide shows once, gated by `Settings::seen_guide`.
- No new deps.

### Library cover-flow carousel rendering

- The Library screen (the two-screen shell) RENDERS the cover-flow carousel from the `Library` model: focused cover with accent ring, scaled/dimmed neighbors, per-cover + focused-meta reading-progress bars, a grayed broken-cover placeholder for unavailable books, and the 0-book empty-state CTA. Built from the pure `library_model::carousel_data` mapping via the `carousel::to_carousel_item` UI-thread adapter (covers via `slint::Image::default()` for now). No new deps.
- **Covers started as placeholders** — "Library carousel covers" (below) streams the real cover images in (into the same `VecModel<CarouselItem>`, using the thumbnail-strip `invoke_from_event_loop` pattern).
- Per-book page `total` was originally a placeholder `0`; **"Per-book page totals, fallible save, and load-failure notice" (below) now persists it** (`Book::page_count`, back-filled and saved on open) and the carousel shows the real `total` + `current / total` progress fraction once a book has been opened. (Later: `CoverController` ALSO prefetches the count for never-opened books in the background, so the carousel no longer shows `1 / 0` before first open — see "Cover sharpness, page-count prefetch, cover-flow z-order" below. Later still: the reject-empty-books feature persists the probed count at ADD time, so a freshly added book shows its real `total` immediately without waiting for the prefetch — see "Reject empty books" below.)
- The empty-state CTA is wired to the file/folder picker (see "Multi-file loading via picker" below).

### Thumbnail disk cache

`ThumbnailCache` (gashuu-core) persists thumbnails/covers as PNG files under the OS cache directory, keyed by a version-stable FNV-1a hash of (path, mtime, max-side). `put` writes atomically (temp-file-then-rename); `get` returns `None` on miss/corrupt. This is the storage primitive; the cover carousel that consumes it is "Library carousel covers" (below). Concurrent same-key write safety is deferred (see docs/patterns.md) and is not exercised by the carousel (one distinct key per book).

### Library carousel covers

- The Library carousel shows each book's REAL cover (the thumbnail of its page 0), streamed in row-by-row rather than placeholders. Core gained `generate_cover(source, max_side) -> Result<DecodedImage, CoreError>` (page-0 thumbnail; errors on a 0-page source); the UI's `cover_loader.rs` (`CoverController`, a twin of the `thumbnail_strip.rs` controller) tries the `ThumbnailCache` first, else fires a `rayon::spawn` worker that opens via `ArchiveLoader`, calls `generate_cover`, caches the result, and marshals it into the shared `VecModel<CarouselItem>` via `invoke_from_event_loop` — under the same epoch + cancel double-guard as the thumbnail strip (see docs/patterns.md). (Originally the cache HIT was served synchronously on the UI thread; the async-cover-loading rework below moved the hit path onto the worker too — `start` is now dispatch-only.)
- `rayon` became a direct dep of the `gashuu` UI crate (no new lockfile entry — see docs/toolchain.md).
- No new lockfile dependencies. Core `generate_cover` is unit-tested (page-0 selection, empty-source error, decode-error propagation); the UI streaming path in `cover_loader.rs` is coverage-exempt like the thumbnail strip (see docs/quality-gates.md).

### Multi-file loading via picker

- **Add files** (`rfd` `pick_files`, filtered cbz/zip/cbr/rar) and **Add folder** (`pick_folder`, folder-as-one-book) toolbar buttons on the Library screen, plus an interactive empty-state CTA that fires the file picker.
- Adds PROBE each source off the UI thread (`add_loader::AddController`, issue 206 — a bulk add of large/cloud-synced archives no longer freezes the event loop; `Adding… (k/N)` progress streams as each probe completes), then APPLY on the UI thread via `apply_outcomes` (dedup via `Library::add`, skipping books already present and duplicates within the batch; since the reject-empty-books feature it ALSO rejects any source with no image pages or that cannot be opened — see "Reject empty books" below), then persist the library, rebuild the carousel model (`build_carousel_model`), and surface the outcome on a Library-screen status line.
- Library is loaded at startup (corrupt/unreadable → empty library, same UI-layer recovery policy as `Settings`) and the carousel is seeded from it on boot.
- No new deps (reuses `rfd`).

### Per-book page totals, fallible save, and load-failure notice

- **Per-book page totals are modeled + persisted.** `Book::page_count` (a `#[serde(default)]` field, no `LIBRARY_VERSION` bump) is back-filled from the opened source's page count and saved to `library.json` on open, so the count survives relaunch. The cover-flow carousel (`carousel_data`) shows the REAL `total` / `current` / progress fraction for any opened book (`0` = unknown: never opened AND not yet prefetched; since the reject-empty-books feature, a book ADDED after that feature lands has its count persisted at add time, so a fresh add shows its real total immediately — see "Reject empty books" below). Same-session visibility comes from a carousel rebuild on the open path when the count was just back-filled.
- **Save is fallible end-to-end** — `Library::to_json -> Result` (symmetric with `from_json`); `save`/`save_to` propagate via `?`. No serialize step is silently swallowed, so a save can no longer write a truncated file while the UI reports success.
- **Startup load failures surface on the home screen.** A genuine `Library`/`Settings` load failure (corrupt data / I/O / `NoDataDir`; missing files still return `Ok(default)`) is collected and shown on the Library status line after the initial refresh. Closes the earlier gap where library load failure was tracing-only.
- No new deps.

### Cover sharpness, page-count prefetch, cover-flow z-order

- **Sharper covers.** `cover_loader::COVER_MAX_SIDE` rises from 160 → **512 px**, decoupled from the page strip's `DEFAULT_THUMB_MAX_SIDE` (160). A focused cover slot is up to 240×336 logical px (≈480×672 physical at 2× Retina), so the old 160 px buffer upscaled ~4× and looked blurry. `max_side` is part of the `cache_key`, so the bump transparently invalidates and regenerates every stale 160 px cover on the next run. The strip path is unchanged.
- **No more `1 / 0` before first open.** `CoverController` prefetches the REAL page count for never-opened books (`Book::page_count_opt() == None`, flagged `needs_count` in `cover_requests`). The cover-MISS worker reuses its archive open to read `list_pages().len()`; a cover-cache HIT resolves the count with one archive open on the same worker, after the cover marshal (originally a separate `spawn_count_only` job — folded into the unified worker by the async-cover-loading rework below). The count is streamed to the row's `total` immediately (`marshal_total`) and queued in `pending_counts` for UI-thread persistence — applied via `Library::set_page_count` + `save` at the next `start` and at shutdown (`flush_counts`), so it survives a relaunch (no re-open needed next time). A worker can't touch the `!Send` `Rc<RefCell<Library>>`, hence the queue + UI-thread drain.
- **Centered cover always on top.** The cover-flow now renders via a file-private `CoverCard` sub-component in TWO `for` passes (backing layer, then the centered card declared after → drawn on top), since Slint 1.x has no per-`Repeater`-item z. Both passes bind identical geometry — since the flow-position rework, pure bindings on the row's single animated scalar — so the Left/Right slide animates as one coherent band even under rapid input (docs/patterns.md "animation altitude"). Fixes the neighbor card overlapping the centered cover.
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
- `.slint` strings live in an `export global Strings` (ui/Strings.slint) with English defaults and are pushed from the Fluent catalog by `Localizer::apply()` (i18n/{en,ja}/gashuu.ftl). Rust-composed strings (status line, open/save notices, decode errors, and the ShortcutsOverlay key-bindings reference) are resolved via `src/i18n/dynamic.rs` using the `fl!()` macro against the same catalog.
- **Fluent is the sole i18n system (ADR-0008, Accepted).** The migration (#112-#115) is complete: gettext, `.po` files, build flags, and `src/messages.rs` were fully excised (#115).

### Continue-reading bookmark and entry focus snap

- `Library.last_opened: Option<PathBuf>` (private, `#[serde(default, skip_serializing_if = "Option::is_none")]`) records the canonical path of the most recently opened book. Set by `register_opened`; cleared by `remove`/`remove_many` when they touch that book; orphan cleared in `Library::normalize()` (normalize-on-load). Accessor: `pub fn last_opened(&self) -> Option<&Path>`. Persisted by the existing `library.save()` leave points; the key is absent until first set (same serde recipe as `last_page`/`overrides`).
- `CarouselData.bookmarked` is a pure derivation (`book.path() == library.last_opened()` in `carousel_data_for_indices`) carried through to `CarouselItem.bookmarked` and consumed by `CoverCard`.
- **BookmarkRibbon** (`ui/components/BookmarkRibbon.slint`): display-only Image atom (`bookmark.svg` — Bookmark Check Fill glyph after the `feat/library-chrome-polish` glyph swap, `colorize: Theme.text` white — a quiet, non-interactive status color borrowed from the typography ladder; not accent, which is reserved for interactive elements), sized `Theme.space-huge²`, floating fully ABOVE the cover at `x: Theme.space-xs; y: -self.height - Theme.space-md` (~10 px clearance ≈ ribbon height / φ²). Zero overlap with the cover art; the dark stage background behind guarantees contrast. Accessible as `accessible-role: image` with label `Strings.continue-reading` ("Continue reading" / 「続きから読む」, both Fluent locales). The SelectionBadge occupies the TOP-RIGHT corner inside the card; both overlays can render simultaneously.
- On entering the Library (app boot and `go_to_library`), the carousel's focused-index one-shot-snaps to the last-opened book resolved through the visible search projection (fallback 0 when `None`, filtered out, or empty). `go_to_library` rebuilds the carousel via `refresh_library_carousel` so the ribbon reflects the current `last_opened` on every re-entry. Search-query changes and add-books do not re-snap (existing focus behavior is unchanged for those paths).
- **NavBar bookmark capsule** (`feat/library-chrome-polish`, 2026-06-05): a `bookmark-icon` `NavItem` in the NavBar (order: search | add capsule(s) | select | bookmark | settings) that jumps to the continue-reading book when clicked. Rust handler reads `Library.last_opened`; if it resolves to a book still in the library, opens it through the same path as Return / a center-cover double-click (resume at saved page); otherwise pushes `notice-bookmark-none` ("No bookmark registered" / 「ブックマークが登録されていません」) to the status strip. Always enabled — the no-bookmark click intentionally produces feedback rather than silently doing nothing.
- **Library bottom strip idle state** (`feat/library-chrome-polish`, 2026-06-05): when `status-text` is empty the `ViewerWindow` bottom strip displays a `library-count-text` — the total book count composed by `library_count_text(loader, n)` in `dynamic.rs` (Fluent plural: "1 book" / "N books" en; "N 冊" ja; empty string when n=0 to avoid noise next to the empty-state panel). Pushed from `refresh_library_carousel` and from `on_set_language`; transient notices always win (`status-text != "" ? status-text : library-count-text`).

**Deferred from this slice:** recently-read sort/history, read timestamps, auto-open on launch, multiple bookmarks.

### Bulk-delete: selection mode, toolbar, and destructive delete (bulk-delete epic, #125–#129)

- The Library carousel has a bulk-**selection** mode. Entered by keyboard `x` (also toggles the focused book) or the NavBar **Select capsule** (filter glyph, toggle + persistent accent ring while active; disabled — not removed — when the library is empty so the NavBar width stays stable); toggled per-book via `x` / Space / a cover click; left via Esc, the toolbar's ✕ exit, or re-clicking the Select capsule. Selection is a `PathBuf` set keyed by canonical path (survives query changes and adds). While selection mode is active a `SelectionBadge` overlays EVERY cover: a filled accent disc + check glyph for selected books; a hollow hairline ring for unselected books (UI-polish two-state). (Selection-mode state + the `x`/Space/cover-click toggles + the `PathBuf` set landed first, #125/#126; the visible toolbar chrome and select-all were added later; the text pill below the NavBar that previously entered selection mode was replaced by the NavBar capsule in `feat/library-chrome-polish` 2026-06-05 — the click-dialog deferral that accompanied the pill's original keep decision is now moot since the capsule is the entry point.)
- **`SelectionToolbar`** organism (below the untouched NavBar, shown only in selection mode with no modal open): a count mode-indicator ("N selected", or "N selected (M outside search)" when some selected books are filtered out of the current search), a **Select all / Deselect all** toggle button, the exit ✕, and the **Delete (N)…** `DangerButton` (the only red element in the app's chrome; if-gated/hidden at N=0). A content-hugging glass pill (no fixed width — see docs/patterns.md).
- **Cmd/Ctrl+A** select-all/deselect-all keyboard chord (selection mode only) — the repo's first `event.modifiers` arm; the same action as the toolbar's Select-all button (Rust decides select-vs-deselect from whether every visible book is already selected). See docs/patterns.md.
- **Delete / Backspace** keys arm the destructive path in selection mode, opening the confirm dialog (same as the DangerButton). `Enter` = Cancel — the destructive action is never on `Enter`.
- **Confirm dialog** (#129): before any deletion, a `ConfirmDialog` is mounted in `ViewerWindow` listing up to 10 book titles (BTreeSet order), an "…and M more" line when the selection exceeds 10, an "N selected outside the current search" line when filtered-out books are included, a warning line when the open book is among the selection, and a "files on disk are kept" info line. Cancel / Esc / backdrop click preserve the selection and mode.
- **Destructive transaction** (`RemoveBooksUseCase`): mutate → save with rollback → best-effort cover purge → viewer-close if the open book was deleted → search recompute → selection clear (success only). **No undo** — the confirm dialog is the safety gate instead. **Source files on disk are never touched** — only the library entry is removed.
- **Save-failure rollback**: if `Library::save()` fails after a `remove_many`, the full `Book` clones (captured before removal) are re-inserted via `Library::restore`, returning the shelf to a byte-identical pre-removal state. The selection is preserved so the user can retry; no cache entries are touched on failure.
- Status line reports "Deleted N book(s)" on success or a loud save-failure message on `SaveFailed`; the focused carousel index is clamped into the shrunken visible-row count.
- The `ShortcutsOverlay` keyboard-reference now documents the full selection grammar: `x`, `Space`, `Cmd/Ctrl+A`, `Delete`/`Backspace`, `Esc`.
- No new dependencies.

### Async cover loading — dispatch-only `start` + focus-first ordering (perf/async-cover-loading)

- **The cover-cache HIT path moved off the UI thread.** `CoverController::start` previously served warm-cache covers synchronously inside its request loop — per book: `fs::metadata` (mtime) + cached-PNG `fs::read` + decode + `to_slint_image`, ~2–8 ms each, so a ~500-book warm start blocked the event loop for seconds. `start` is now DISPATCH-ONLY: every request becomes one rayon job (`spawn_load`), and the worker derives the cache key (mtime read included), serves the hit or generates on miss, and marshals the cover back via the existing epoch-guarded `marshal_cover`. Hit and miss share one worker path — the same shape as the thumbnail strip, ending the controllers' execution-model asymmetry.
- **`spawn_count_only` was folded into the unified worker**: a HIT row that still `needs_count` resolves it on the same worker (one archive open, after the cover marshal so the count never delays the visible cover).
- **Focus-first dispatch.** `prioritize_by_focus(requests, focus_row)` (pure, unit-tested) reorders requests by `abs_diff` from the carousel's focused row before dispatch, so the covers the user is looking at stream in first on a large library; `refresh_library_carousel` reads `carousel_focused_index` after its reset-focus step.
- **`pending_counts` element is the named `ResolvedCount { path, count }`** (was a bare `(PathBuf, NonZeroUsize)` tuple).
- `start` logs `dispatched` + `elapsed_us` at debug level — the UI-thread cost of a refresh is now measurable and independent of cache state.
- Follow-ups deliberately NOT done here (filed as issues): #140 page-turn miss-decode analysis, #141 raw-RGBA/QOI cover-cache v2, #142 batched cover marshaling, #143 ThumbnailCache size cap/GC (since shipped — see the cover-cache GC section below), #144 failed-cover state.
- No new dependencies. The worker path stays coverage-exempt (same policy as the thumbnail strip); `prioritize_by_focus` is unit-tested headlessly.

### Async page decode (issue #207)

- **Cache-miss page decode moved off the UI thread.** Previously `main.rs::refresh` called
  `ViewerState::current_spread()`, which decoded missing pages synchronously on the event loop —
  blocking every turn for however long the decode took. `refresh` now calls
  `ViewerState::spread_slots()`, which is a non-blocking HIT/MISS classification: it returns each
  slot's index plus a cached `Arc<DecodedImage>` on a HIT or `None` on a MISS, never reading or
  decoding on a miss.
- **All-HIT path (sync apply).** When every slot is a HIT, `refresh` applies the images
  synchronously — clears the loading flags, builds the `slint::Image` objects on the UI thread, and
  calls `apply_spread_images` + `apply_spread_geometry`. No rayon job is spawned.
- **Any-MISS path (async).** On any MISS, `refresh` sets `leading-loading` / `trailing-loading` to
  show the per-slot loading placeholders, clears the image properties, and calls
  `PageController::dispatch_spread`. `dispatch_spread` reserves each missing slot independently
  (dedup prevents duplicate in-flight decodes for the same page), spawns one rayon job, and uses
  `rayon::join` when both slots are MISS so they decode in parallel. The job marshals back via
  `slint::invoke_from_event_loop`, epoch-guarded: on success it calls
  `ui.invoke_spread_anchored(content_w, content_h, single, trailing_failed, leading_idx,
  trailing_idx)`; on decode failure it calls `ui.invoke_page_decode_error(index)`.
- **Epoch + dedup.** `PageController::set_target` advances the epoch on a real spread change; stale
  marshal closures arriving after a navigation detect the superseded epoch via `is_current` and
  return without touching UI state. `reserve_dispatch` deduplicates: if a slot is already in-flight
  from a prior dispatch it is not re-dispatched, and the earlier worker's marshal completes it.
- **`current_spread()` retained for tests only** (`#[cfg(test)]`). It decodes synchronously and is
  never called on the UI thread in production.
- No new dependencies.

### Reject empty books (reject-empty-books, 2026-06-05)

A source with no image pages (an empty folder, an archive with only non-image entries, a folder whose images live only in subfolders since the walk is `max_depth(1)`) is no longer treatable as a book. The domain rule "a valid book has >= 1 image page" lives once in core; three UI hook points enforce it. See [ADR-0009](ADRs/0009-reject-empty-books.md) for the decision and [docs/patterns.md](patterns.md) for the harnesses.

- **The rule lives in core.** `ArchiveLoader::probe_page_count(path) -> Result<NonZeroUsize, CoreError>` opens the source and counts pages; `0` → `Err(CoreError::EmptyBook { path })` (a new `#[non_exhaustive]` variant), `1+` → `Ok(NonZeroUsize)`. I/O / `UnsupportedFormat` errors propagate UNCHANGED — "empty" and "unreadable" are strictly distinct. `Library::add` / `register_opened` are unchanged (no I/O enters the collection layer).
- **Add time.** The probe (`add_loader::probe_path`, off the UI thread) classifies each path and `apply_outcomes` rejects empty OR unreadable sources before they enter the library (the notice says how many were skipped; mixed batches still add the valid books). On a genuine insert it persists the probed count via `set_page_count`, so a fresh add shows "1 / N" immediately. Return type is `AddReport { added, skipped }`.
- **Open time.** `OpenBookUseCase::run` returns `OpenOutcome::EmptyBookRemoved { title, removed, save_error }` for a clean open that counts zero pages: it removes the book (if present), re-saves, purges its cover (Wave-2 #150 closed the old open-path purge gap), and the recents push / settings save / `register_opened` are all bypassed; `main.rs` stays on the Library, rebuilds the carousel, and shows a notice (never enters the viewer).
- **Cover-load time.** A cover worker that opens a book cleanly with zero pages fires the `empty-book-detected(string)` Slint callback; `main.rs::on_empty_book_detected` runs the shared `app::remove_empty_book` transaction (remove → save → cover purge), rebuilds the carousel, and notifies. Idempotent with the open-time path via `Library::remove`'s bool (a race loser stays silent).
- **A missing-path book is NOT removed** — the existing `is_available()` gray-out is preserved (a temporarily unmounted drive must not lose data). Removal happens only when a scan SUCCEEDS and confirms zero images.
- **A FAILED open stays on the Library (PR #334, 2026-07-01).** When a carousel/bookmark open returns `OpenOutcome::Error` — not empty, not removed (e.g. the file moved or its volume is unmounted) — it no longer enters a blank 0-page Viewer: `open_and_enter` gates `enter_viewer` on `OpenOutcome::Success` only, and on `!path.exists()` shows the book-named `viewer-open-inaccessible` status message ("『…』を開けませんでした。ファイルが見つからないか、保存先のボリュームに接続できません。" / "Couldn't open …. The file is missing or its volume isn't connected.") instead of the raw I/O error. A failure with the file still present (a corrupt archive) keeps the detailed error. This is the missing-drive complement to the rule above.
- Notices: `notice-added-books-skipped`, `notice-no-books-added-empty`, `notice-empty-book-removed` (en + ja, via `i18n/dynamic.rs`).
- No new dependencies. Add-time probing is synchronous on the UI thread (light: zip reads only the central directory, folder probing is a shallow walk); a follow-up to move it off-thread is deferred (YAGNI) unless it proves slow on huge network-drive batches.

### Cover-cache GC — size cap + near-LRU prune (#143, 2026-06-05)

The on-disk cover cache is now bounded. Core's `ThumbnailCache::prune(max_bytes) -> PruneReport`
sweeps the cache directory down to the cap in ascending `(mtime, file name)` order and reclaims
stale `.{key}.tmp` crash leftovers (older than 1 h, age-guarded so in-flight writes survive);
`get` refreshes a hit's mtime (touch-on-get, after a successful decode only), making eviction
near-LRU so mtime-orphaned covers — never read again — age out first. The cap POLICY is the app
layer's: `cover_loader::COVER_CACHE_MAX_BYTES` (256 MiB) + `spawn_cache_prune()`, one rayon job
dispatched at startup after the initial cover stream (no UI-thread I/O). `purge_for` semantics
unchanged. Deferred: re-pruning inside a session (overflow waits for the next launch), a Settings
field / UI for the cap.

### Data-clearing cleanup controls; Private Mode dropped (issue #178, branch `refactor/add_private_mode`, 2026-06-08)

- Two confirmation-free cleanup buttons on the Settings dialog's General tab. **Clear reading
  history** empties the whole library — `Library::clear` clears `books` + `last_opened` and the
  recent-files list (in gashuu the library IS the reading history; a book is shelved on first open)
  — saves library and settings independently, and rebuilds the carousel; a book open in the current
  session stays open. **Clear cover cache** deletes the on-disk thumbnail/cover files via
  `ThumbnailCache::clear` (best-effort, like `prune`: non-recursive, no symlink-follow, owned
  `*.png` / `.*.tmp` only, missing dir → zero report) and reports the file count + reclaimed bytes.
  Both surface localized feedback through a transient in-dialog status line (`data-action-status`),
  reset on dialog open, no timer. See [docs/patterns.md](patterns.md).
- **Private Mode was explored and fully prototyped, then DROPPED by user decision.** A book add in
  gashuu is explicit curation, not a passive reading trace, so the privacy model did not fit; only
  the two cleanup controls above survived. No new dependencies.
- **Implemented on branch `refactor/add_private_mode`, pending merge** (not yet shipped on `main`).

---

## Deferred (intentionally out of scope)

- A genuinely-RAR-compressed test fixture (issue #22 — only a store-format fixture ships, which does not exercise real RAR decompression).
- Progressive per-slot double-spread display (rejected — per-slot geometry would fracture the unified zoom/pan content rectangle; the spread applies atomically).
- Thumbnail-strip follow-ups:
  - RTL strip ordering (the strip ships ascending order + current-page highlight only).
  - Virtual scroll for huge archives — only *decode* is lazy/on-demand (see "Parallel thumbnail strip"); the model still materializes one cell per page (`(0..page_count)` `set_vec`), so the row model is not yet windowed.
- Nested archives.
- `ComicInfo.xml` metadata.
- Password-protected ZIP/RAR.
- Per-entry offset/cursor-cache optimization (avoid reopening the central directory / re-walking from the front on every `read_bytes`).
- Multi-volume/split RAR.
- Solid-RAR skip-speedup.
- User-remappable keys (`key_bindings` stays persisted-but-inactive; the settings dialog shows a read-only reference).
- Immediate runtime rebuild of the CURRENT book's cache (`ViewerState::rebuild_cache` — `set_cache_config` only affects newly opened books).
- `recent_files`-management / theme settings UI.
- The three multi-file-loading follow-ups SHIPPED (see "Per-book page totals, fallible save, and load-failure notice" above): on-screen library-load-failure notice, fallible `to_json` (no silent serialize-error discard on save), and real carousel `total`/`progress` via persisted page counts. Covers now stream in (see "Library carousel covers" above).
- Backdrop-click / Esc dialog dismissal (settings dialogs close via their own button only).
- Viewer non-goals: rotation/minimap/scrollbar, per-page independent zoom in Double mode, and 60fps is NOT CI-asserted (manual/telemetry only). Input that DID ship is documented above (see the spread/navigation and zoom/pan sections): click/tap-to-turn, two-finger trackpad swipe-to-turn (at fit), native trackpad/touch pinch zoom (`ScaleRotateGestureHandler` → `ViewportState::pinch_to`), keyboard zoom (`+`/`-`), and drag- and scroll-panning via `ViewportState`. Rotation from the pinch gesture is intentionally ignored.
- Linux release artifacts and a `.desktop` entry (macOS + Windows ship via `release.yml`; Linux's Slint system-library deps make a portable artifact heavier — deferred).
- Code signing / notarization for release binaries (macOS Developer ID + notarytool, Windows Authenticode) — `release.yml` ships unsigned with documented `SIGNING SEAM` insertion points.
- Auto-creating the GitHub Release (the release must pre-exist; `release.yml` only uploads assets to it).
