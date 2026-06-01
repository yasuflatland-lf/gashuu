# Patterns & gotchas (learned the hard way)

This is the L2/L3 reference doc migrated from the CLAUDE.md "Patterns & gotchas" section.
An agent should read the relevant entry BEFORE editing the corresponding code area.

### Cross-crate mocking via a `testing` feature

`gashuu-core` gates `mockall::automock` on `PageSource` behind `[features] testing = ["dep:mockall"]`; `gashuu`'s dev-dependency enables it, so `ViewerState` tests use `MockPageSource` without pulling `mockall` into release builds.

### `#[allow(dead_code)]` on test-only accessors

In a *binary* crate `pub` is not a public API surface, so `-D warnings` flags an accessor used only by `#[cfg(test)]` code as dead; such `#[allow(dead_code)]` is intentional and documented in place. (PR8a's thumbnail-strip wiring now USES `ViewerState::page_count()`/`index()` at runtime, so they shed their `#[allow]` — the pattern still applies to any future test-only accessor.)

### A pure Rust helper is the tested spec for logic re-derived in Slint markup (PR-S)

When a mapping is computed in Slint expressions at runtime (e.g. the scrubber knob-fraction → page), keep a pure Rust twin (`scrub_fraction_to_page`) as the unit-tested authoritative spec and keep the Slint expression an EXACT mirror: same clamp to `[0,1]`, same round-half-up (`floor(x + 0.5)`), and RTL inverts the fraction BEFORE rounding (`floor((1-frac)*last + 0.5)`) — `last - floor(frac*last + 0.5)` diverges at half-integer fractions. The twin has no runtime caller in the binary, so it carries `#[allow(dead_code)]` (see the entry above). Cross-reference both sides so a future edit to one stays in lockstep with the other.

### Enforce load-bearing invariants in the type, not in prose

`DecodedImage` keeps `rgba`/`width`/`height` private with a checked `new() -> Result<_, CoreError>` (validates `rgba.len() == width*height*4`, else `CoreError::MalformedImage`); public fields would let a caller build a value that panics `copy_from_slice` in `to_slint_image`. Construct via `new`; read via `width()/height()/rgba()`.

### Decode with limits (two-layer)

`image_ops::decode` first does a header pre-read with a SECOND lightweight `ImageReader` (`into_dimensions()` consumes the reader, so a second one is required) → `check_pixel_limit(w,h)?` (pure, no alloc; `MAX_PIXELS`=128 Mpx aligns with the 512 MiB / 4-bytes-per-RGBA cap; `CoreError::ImageTooLarge {width,height,pixels,max}`), THEN the full decode via `image::ImageReader` + `image::Limits` (16384×16384, 512 MiB alloc cap) to reject decompression bombs before allocating. `image::Limits` is `#[non_exhaustive]`, so build it with `Limits::default()` + field assignment (hence the local `#[allow(clippy::field_reassign_with_default)]`).

### `DynamicImage::thumbnail` UPSCALES

small images to fill the bounding box — it is NOT downscale-only despite the name. `decode_thumbnail` guards with `if width > max_side || height > max_side { thumbnail() } else { unchanged }`, so a source already within `max_side` is returned at its original size. Discovered empirically in PR8a — a no-upscale test pins it and the guard's comment credits this, so nobody deletes the guard.

### Don't swallow `WalkDir` errors

`FolderSource::open` counts unreadable entries into `skipped_count()` rather than `.filter_map(Result::ok)`; the UI (`ViewerState::open_folder`) logs them via `tracing::warn!`. Core stays logging-free while the failure still surfaces.

### Slint focus after a Button click

Clicking a `Button` moves focus to it; the page `FocusScope` must call `fs.focus()` after the action (and on `init`) or keyboard navigation silently stops working.

### Clear the displayed page on error

`refresh` clears `current-page` to `slint::Image::default()` on an empty folder and on a decode error, so the view never shows a stale page that contradicts the status text.

### Guard non-object JSON before `migrate`

`Settings::from_json` must reject non-object roots (`5`/`[]`/`"x"`/`true`/`null`) BEFORE `migrate()`, which indexes the `serde_json::Value` as a map and PANICS on a non-object. A panic bypasses the UI's `unwrap_or_else` recovery → startup crash on a hand-edited file. Guard with `!value.is_object()` and deserialize into `serde_json::Map` to surface a typed `Err`. **Do NOT guard with `from_value::<Settings>`** — every field is `#[serde(default)]`, so serde will deserialize a JSON array positionally and silently return defaults instead of erroring.

### Enforce read-path invariants on load, but only the ones with no valid alternative

`from_json` normalizes after deserialize: `cache_size.max(1)` (mirrors `ImageCache::new`'s coercion so the stored value matches the value actually used) and `recent_files.truncate(MAX_RECENT_FILES)` (an over-long hand-edited list would otherwise persist forever via exit-save). **`preload_pages` is deliberately NOT clamped** — 0 is a valid "prefetch disabled" radius and not coerced downstream, so clamping it would silently override a legitimate user choice.

### Parse the schema `version` with `u32::try_from`, not `as u32`

a truncating cast wraps crafted huge values (`u32::MAX + 1` → 0) and silently re-migrates.

### insta snapshots use `assert_snapshot!` (plain string; no `json` feature)

The generated `.snap` is committed text (not a binary fixture). Generate/refresh with `INSTA_UPDATE=always mise exec -- cargo nextest run -p gashuu-core`; CI never updates snapshots, so a `.snap` mismatch fails the build — the freeze is enforced automatically. Keep snapshot inputs deterministic (`Settings::default().to_json()` — no absolute paths or timestamps). PR4 added `cover_mode:"standalone"` and PR5 added `fit_mode:"whole"` to the snapshot (`reading_direction:"ltr"`/`spread_mode:"single"` unchanged). The default snapshot is unchanged by PR4a (default is still `single`); `"auto"` round-trip serialization is covered by a separate string assert, not the snapshot.

### `Settings` uses pub serde fields, not a checked constructor

Its invariants are semantic (`cache_size>=1`), not physical like `DecodedImage`'s `copy_from_slice` panic — and a checked constructor would force `Default` to return `Result`. Invariant logic is centralized in `push_recent` (dedup + most-recent-first + `MAX_RECENT_FILES` cap) and the load-path normalization described above.

### `SpreadMode::Auto` is a NEW persisted variant (PR4a)

a `settings.json` written by this build (`spread_mode:"auto"`) cannot be read by a pre-PR4a build — that build will reject the unknown variant and fall back to defaults via the existing `unwrap_or_else` + `tracing::warn!` recovery. This is intentional/accepted; no `SETTINGS_VERSION` bump was made (bumping would change the frozen snapshot and would not grant true downgrade safety).

### `spread_mode`/`reading_direction` (PR4), `SpreadMode::Auto` (PR4a), and `fit_mode` (PR5) are ACTIVE; only `key_bindings` stays persisted-but-inactive

PR4 activated the spread settings and rewrote `keymap::map_key` to take a `dir: ReadingDirection` and emit a `KeyCommand` (arrows resolve against the active direction). PR4a added `SpreadMode::Auto`, resolved via `SpreadMode::resolve(window aspect)` at the UI layer (`ViewerState::effective_layout()`) into a `SpreadLayout` before every pairing call; pairing functions take `SpreadLayout` so `Auto` is unreachable in pairing by type. PR5 wired `fit_mode` to real behavior (persisted, forward-compat like `cover_mode`). `key_bindings` is still saved for forward-compat only: `KeyBindings`'s default tokens match what `map_key` hard-codes, but `map_key` does NOT read the struct — user-remappable keys remain deferred.

### A new/changed key binding must be updated in BOTH places that describe keys (PR-0b)

`keymap::map_key` decodes the token to a `KeyCommand`, and `main.rs`'s `KEY_BINDINGS_HELP` const is the in-app/settings key reference shown to the user. They must stay in sync (the const's own doc says so). Adding a binding in only one place leaves the user-facing help contradicting real behavior.

### Separate pairing / placement / input

`spread.rs` decides WHICH pages pair (reading order) and holds NO `reading_direction` and NO `NavAction` (no core→UI type leak) — so the decision table doesn't double over direction. Pairing functions receive an already-resolved `SpreadLayout` (never `SpreadMode`/`Auto`); the only `SpreadMode → SpreadLayout` conversion is `SpreadMode::resolve`. Placement (RTL = `HorizontalLayout` slot reversal in `PageView.slint`) and input (which arrow advances, resolved by `reading_direction` in `keymap::map_key`) live in the UI. `NavAction {Next,Prev}` stays reading-order as the single source of truth.

### Spread is a derived value, not stored state

`ViewerState` keeps only `index` (= current spread's leading page) + the modes; the spread is recomputed each call via `spread_at` (avoids dual-source drift). Invariant: `index` is ALWAYS a valid spread-start — reset to 0 on `set_source`, mutated only via `next_/prev_leading`, and re-anchored by `normalize_leading` after a `spread_mode`/`cover_mode` toggle so the visible page stays on screen. `reading_direction` toggles do NOT normalize (pairing is direction-agnostic).

### `ViewerState::set_viewport_size(width, height) -> bool`

updates `viewport_aspect` and returns `true` ONLY when the effective `SpreadLayout` actually flips (so `auto` mode causes no churn while resizing within the same layout). On a flip, `normalize_leading` re-anchors the index so the current page stays visible. `main.rs` calls `refresh` only when `set_viewport_size` returns `true`. `auto` resolves aspect `>= 1.0` as Double. The stored `viewport_aspect` is sanitized at storage — any `width/height` ratio that is non-finite or non-positive is coerced to `1.0` (→ Double), so the field always holds a valid ratio; `SpreadMode::resolve` repeats the same guard as a standalone safety net. The `D` toggle is now a 3-cycle (single → double → auto) handled in `ViewerState::toggle_spread`; `keymap` still just returns `ToggleSpread`.

### `CoverMode {Standalone(default), Paired}` controls cover layout in Double mode only

(ignored in Single): Standalone = cover index 0 alone, then `{1,2}{3,4}…`; Paired = `{0,1}{2,3}…`. Default Standalone is the manga convention.

### `PageView` takes a Rust-computed `single` bool

(= `trailing.is_none()`), not an in-Slint empty-image check — detecting an empty `image` in Slint is version-fragile, so the single/double decision is made in Rust and passed as a bool. `rtl` reverses the two image slots.

### Trailing-page decode failure degrades to single, never silent

`current_spread` propagates a LEADING decode error (`Some(Err)`), but on a TRAILING error it logs `tracing::warn!`, sets `trailing=None` AND `trailing_failed=Some(page)`, and `refresh` appends a `(page N unavailable)` status marker so the status never contradicts the single page shown (the documented "view must match status" rule).

### `CoreError` is `#[non_exhaustive]`

so later PRs can add variants without breaking matches (`ImageTooLarge` joined in PR5, the archive variants in PR6 — all non-breaking).

### Zoom/pan geometry split mirrors `spread.rs`

Pure fit-scale / pan-clamp / cursor-anchored-zoom live in `gashuu-core/src/viewport.rs` (stateless, table-tested, NO Slint/tracing); the live zoom/pan/fit + viewport size live in UI `gashuu/src/viewport.rs` `ViewportState`, which delegates ALL clamping to the core fns. WHY: keeps clamp math unit-testable and out of Slint expressions.

### Effective scale = `clamp_zoom(zoom) * fit_scale(...)` is composed by the UI caller, not core

core exposes the pieces separately (no combined helper); `ViewportState` has a private `fit()` baseline helper.

### Two-statement RefCell borrow in Slint zoom/pan callbacks

Mutate via `borrow_mut()` in ONE statement (the temp borrow drops at the `;`), THEN take a fresh `borrow()` to pass `&ViewportState` into `apply_viewport` — never hold `borrow_mut` across the apply call (the borrow is at the call site). Avoids a double-borrow panic.

### Image-bomb guard is defense-in-depth

(see the two-layer "Decode with limits" bullet): the early `check_pixel_limit` rejects via `CoreError::ImageTooLarge` BEFORE the `Limits`-bounded full decode, with no allocation.

### Test `decode()`'s oversized rejection WITHOUT allocating

Encode a tiny valid PNG, patch the IHDR width/height bytes to oversized dims, and RECOMPUTE the IHDR CRC-32 (poly 0xEDB88320 over chunk-type+data); `into_dimensions()` reads IHDR only. Assert the `ImageTooLarge` variant (NOT `Decode(Limits)`) — that proves the EARLY `check_pixel_limit` line rejects it, which the pure-function unit test alone cannot guard.

### `fit_scale` returns 1.0 on non-positive inputs (intentional zero-div guard)

and `refresh` legitimately calls `set_content(0.0, 0.0)` on the decode-error / empty-folder paths (view-matches-status). Do NOT add non-negative `debug_assert`s to `fit_scale`/`set_content`/`resize`: they would panic on this legitimate zero path.

### Wheel zoom uses sign-only normalization (platform-independent)

`step` = `ZOOM_STEP`(=1.1) / `1/ZOOM_STEP` / `1.0` by the sign of the raw delta — magnitude ignored. Convention `raw_delta>0`=zoom-in; the platform flip point is documented in the Slint `on_zoom_at` callback. Keyboard `+`/`-` anchors at the viewport CENTER; the wheel anchors at the cursor.

### Slint zoom/pan plumbing

Rust computes the displayed content rect (`content-x/y/w/h`) placed inside a `clip:true` `PageView` root (NOT an in-Slint fit — version-fragile). `e.delta-y / 1px` converts a `length` to the callback's `float`. `TouchArea` is non-focusable so it doesn't steal keyboard focus (keep `fs.focus()`). Double-spread content box = `(lead.w+trail.w, max(h))`; the `HorizontalLayout` splits `content-w` into equal halves (1:1 stretch), each image contain-fit (letterbox/pillarbox for mismatched sizes; exact for equal-size pages).

### `fit_mode` is persisted (forward-compat, like `cover_mode`); zoom & pan are session-only

`SETTINGS_VERSION` stays 1 (`#[serde(default)]` absorbs the field). `f`=cycle fit / `1`=actual mutate ONLY `ViewportState` (the runtime owner of `fit_mode`); `reconcile_settings` mirrors it into `Settings` at the next save (PR-D / issue #32, no per-key `Settings` write). Changing fit / `0`(reset) resets zoom to 1.0; a page turn keeps zoom+fit and only re-centers pan.

### Zoom/fit keys (`+`/`=`, `-`, `0`, `1`, `f`) are direction-INDEPENDENT

(unlike arrows); `KeyCommand` gained `ZoomIn/ZoomOut/ResetView/FitActual/CycleFit`. `ResetView` resets zoom but NOT `fit_mode`.

### `ViewportState` invariants are procedural, not type-encoded

Every mutating method ends in a clamp (`zoom` ∈ [ZOOM_MIN,ZOOM_MAX]; offset re-clamped), and `geometry()` applies a final defensive clamp before returning to Slint. A `Zoom` newtype would over-complicate given `Default`. Private fields + intent-named methods.

### `ZipSource` is lock-free: each `read_bytes` opens its OWN `File` + `::zip::ZipArchive`

So rayon prefetch threads decompress fully in parallel with NO shared mutable state; resident RAM is one entry per in-flight read (NOT a single page under concurrent prefetch). Rejected alternatives: a shared `Mutex<ZipArchive>` would serialize prefetch back into single-threaded decode; an `Arc<[u8]>` whole-archive buffer would pin ~1 GB resident for a large CBZ. Trade reopen cost for parallelism.

### Two-tier per-entry 500 MB ceiling (`MAX_ENTRY_BYTES`) defends size-spoofing zip bombs

`MAX_ENTRY_BYTES` lives in `naming.rs` (PR7 moved it there from `zip.rs`; it is an archive-entry-domain property shared by BOTH `ZipSource` and `RarSource`). Open-time (both sources): skip entries whose DECLARED size > max. Read-time for `ZipSource`: `Read::take(max+1)` then `buf.len() > max` → `EntryTooLarge` — the read-time `take` is the REAL cap (a crafted header can lie); `with_capacity(size.min(max))` is only a growth hint, not a trust boundary. **`RarSource`'s read-time cap is WEAKER** — see the RAR bullet below.

### zip-slip defense + corrupt-entry policy is skip+count, container failure is hard-fail

Entries where `enclosed_name() == None` (path traversal) are skipped and counted — but only image-looking ones are counted, so the surfaced "skipped N" is meaningful (in-memory extraction means no disk write, so no zip-slip *write* hazard exists; the skip is hygiene). A per-entry `by_index(i)` error in the open loop (corrupt central-directory entry, or — a side benefit of deflate-only — an entry compressed with an unsupported method like bzip2/lzma/zstd) is ALSO skip+counted, never propagated and never silent garbage. But `ZipArchive::new(...)?` (a fundamentally broken container) STILL hard-fails with `CoreError::Zip`.

### Refer to the `zip` crate as `::zip::` inside `page_source/zip.rs`

the local module is also named `zip`, so the extern-prelude name is shadowed; the leading `::` reaches the crate.

### `ZipSource` intentionally does NOT derive `Debug`

(matches `FolderSource`, and `Arc<dyn PageSource>` is not `Debug` either) — so error-path tests assert via `let Err(..) = .. else { panic!() }`, not `unwrap_err()`/`expect_err()`.

### `PageEntry::name` for `ZipSource` is a LOGICAL archive entry name

(e.g. `sub/3.png`), not a real FS path — display/identity only. `PageEntry` carries `name` only; it has NO `path` field (PR-C / issue #31 removed it). `FolderSource` keeps real FS paths in a private internal `FolderEntry { path, name }`, used only by its own `read_bytes`. Bytes are always retrieved via `read_bytes(index)` keyed on the `zip_index`, never by opening a path.

### Test the two-tier size ceiling via private seams, not a 500 MB fixture

`open_with_limit(path, max)` / `read_entry(index, max)` let the limit be exercised deterministically with a tiny archive (same "exercise the synchronous core" philosophy as cache `radius = 0`). CBZ fixtures are synthesized in a tempfile via `::zip::ZipWriter` + `SimpleFileOptions` + `CompressionMethod::Stored` (predictable byte length) — **no committed binaries** (same rule as folder PNG synthesis; core dev-deps already have `tempfile` + `image`).

### The UI crate (`gashuu`) deliberately has NO `tempfile`/`zip`/`base64`/`rar` dev-dep

so `ViewerState::open_path` tests (CBZ and PR7's CBR alike) use the error-path/default-state strategy; CBZ/ZipSource AND CBR/RarSource correctness is owned by core's `zip.rs`/`rar.rs`/`archive_loader.rs` tests.

### `RarSource` is lock-free via reopen + sequential-skip (RAR has NO random access)

`unrar`'s typestate API processes entries strictly front-to-back — there is no `by_index`. So each `read_bytes` opens its OWN `::unrar::Archive` + `open_for_processing()`, then `read_header()`/`skip()` walks forward to the target's `seq_index` before `read()`. No shared mutable state → rayon prefetch threads each own an independent handle; resident RAM = one page (stable on a 500 MB CBR). The O(N) skip is cheap on a non-solid CBR (it skips past compressed data); solid archives pay decompression on each skip (accepted; a cursor-cache optimization is deferred). Mirrors `ZipSource`'s lock-free reopen but trades random access for a sequential walk.

### `seq_index` invariant is the load-bearing RAR correctness property, enforced by `debug_assert`

Each `EntryMeta.seq_index` is the 0-based position in the FULL sequential header stream (counting directories AND non-images). Listing (`open_for_listing`) and processing (`open_for_processing`) traverse the same archive in the same order, so the index is stable across the two passes — `read_entry` `debug_assert_eq!`s that the entry reached at `seq_index` has the same `enclosed_name` as the listed `meta.name`, turning a listing↔processing desync (silent page-misnumbering) into a loud dev/test failure. (A `SeqIndex` newtype would be over-engineering — the assert is the idiomatic guard here.)

### RAR per-entry listing error = skip+count+`break` (NOT interior-skip, NOT whole-archive hard-fail)

`unrar`'s `List` iterator is NON-RESUMABLE: after any per-entry error it sets `damaged` and yields `None` forever, so (unlike `ZipSource`'s random-access `by_index` skip+count of an INTERIOR entry) RAR can only drop the TRAILING remainder. The open loop therefore does `Err => { skipped += 1; break; }` — surfacing the good pages already indexed + counting the failure (skip+count ethos as far as the format allows). A fundamentally broken CONTAINER still hard-fails at `open_for_listing()?` before the loop. NUANCE: `unrar` emits a phantom `Ok("")` (empty filename) right before the terminal `Err` on a corrupt trailing header; it is filtered as neither a page nor a skip (empty name → `!has_image_ext`). This is an intentional, documented divergence from ZIP's interior skip+count.

### `RarSource`'s read-time size cap is WEAKER than `ZipSource`'s (no streaming `take`)

`unrar`'s `read()` materializes the WHOLE entry into a `Vec` with no streaming seam, so RAR's read-time check only RE-VALIDATES the declared `unpacked_size` against `MAX_ENTRY_BYTES` — it guards against the entry changing between listing and reading, NOT against a header that under-reports its size. `image::Limits` in `image_ops::decode` is the final backstop. Accepted weaker guarantee, documented at the call site.

### `unrar` 0.5.x API gotchas (verified at impl time)

`Archive::new(&path)` borrows; `open_for_listing(self)`/`open_for_processing(self)` CONSUME self (reopen per operation). Listing yields `Result<FileHeader, UnrarError>`; `FileHeader { filename: PathBuf, unpacked_size: u64 }` + `is_directory()`. Processing typestate: `read_header()? -> Option<cursor>` (`None` = end → mapped to `IndexOutOfRange` "file changed under us"), `cursor.entry() -> &FileHeader`, `cursor.skip()`, `cursor.read() -> (Vec<u8>, rest)` — **bytes are the FIRST tuple element** (`let (data, _rest) = cursor.read()?`). The error type is `::unrar::error::UnrarError` (impls Error+Display → `#[from]` works). A MISSING file surfaces as `CoreError::Rar` (`unrar` opens the file itself), NOT `CoreError::Io` — UNLIKE `ZipSource` (whose `File::open` yields `Io`). The local module is `rar` and the crate is `unrar` — DIFFERENT names, so no shadowing (contrast PR6's `zip` module vs `zip` crate that needed `::zip::`); `::unrar::` is used for clarity, not necessity.

### RAR fixtures are hand-written RAR4 STORE-format (method 0x30, uncompressed), base64 TEXT in `test_fixtures.rs`

There is no Rust RAR encoder, so a store-format generator emits just a container (no proprietary RAR compression) and the result is embedded as `pub(crate)` base64 constants in `#[cfg(test)] mod test_fixtures` (declared in `lib.rs`). Three fixtures: (A) distinct per-page DIMENSIONS + an explicit `sub/` directory header + a SCRAMBLED physical order so natural-sort genuinely reorders (`page_index != seq_index`, the only thing that actually exercises the sequential-skip walk — a fixture whose physical order equals natural order is a no-op test); (B) hostile (`../evil.png` + `../readme.txt` traversal → the image-looking one is skip+counted, the non-image isn't); (C) corrupt-trailing (drives the skip+count+`break` path). Store mode does NOT exercise real RAR decompression — that is deferred to PR7a (issue #22): replace with a real WinRAR-compressed fixture.

### Surface skipped count in the status bar for BOTH folder and archive opens

`ViewerState::last_open_skipped()` + `main.rs` appending it (after `refresh`, via `get_status_text`/`set_status_text`). WHY: `tracing::warn!` alone is invisible in a GUI run (`RUST_LOG` is usually unset).

### `ArchiveLoader` dispatch is `ext_kind` (no I/O) → `magic_kind` sniff (PR7 replaced the old `is_zip`/`read_exact` probe)

`magic_kind` does ONE bounded `read` into a 6-byte buffer (sized to the longest magic, RAR's), NOT `read_exact` — a short file yields a small `n` and the `filled.len() >= 4` (ZIP, 4-byte `PK` signatures) / `>= 6` (RAR) length guards treat too-few-bytes as "no match" → `UnsupportedFormat`; only a genuine I/O error propagates. The RAR magic `Rar!\x1A\x07` is the 6-byte prefix shared by RAR4 (`…\x00`) and RAR5 (`…\x01`); the differing 7th version byte is deliberately NOT tested, so one constant matches both.

### Background prefetch is fire-and-forget on rayon over `Arc<Mutex<LruCache>>`

Cache hits must stay instant (clone an `Arc`, never block on prefetch). Locks are released before the parallel decode section, so mutexes cannot be poisoned in practice — `lock().unwrap()` calls are intentional fail-fast, documented at the `Inner` struct.

### Lock order is `cache` → `in_flight`

whenever both are held; `get` only ever takes `cache`. Violating this order risks deadlock — never reverse it in new code.

### Clean up reserved shared state with an RAII guard; `Drop` must never `.unwrap()` a lock

Use `unwrap_or_else(|e| e.into_inner())` to recover a poisoned lock, or a panic during unwind becomes a double-panic abort. `InFlightGuard` exists so a panic in the decode section cannot permanently leak in-flight markers (which would silently disable prefetch for those pages).

### `get`/`current_image` return `Arc<DecodedImage>`

so cache hits never copy the multi-MB RGBA buffer; the UI's `to_slint_image(&DecodedImage)` is unchanged thanks to deref coercion (`&Arc<DecodedImage>` → `&DecodedImage`).

### Verify trait thread-safety at compile time

A `#[cfg(all(test, feature="testing"))]` test asserting `fn assert_send_sync<T: Send + Sync>()` over `FolderSource` and `MockPageSource` locks in the `Send + Sync` supertrait — if a future `PageSource` impl breaks it, the crate won't compile.

### Test async caches deterministically by exercising the synchronous core

Cache-semantics tests use `radius = 0` so rayon tasks are inert; `prefetch_indices` (pure) and `Inner::prefetch_blocking` (sync) are tested directly; the in-flight skip branch is tested by pre-seeding `in_flight`. Never assert on wall-clock timing — the `<50 ms` page-turn target is observed via `RUST_LOG=debug` `tracing::debug!(elapsed_ms=…)` in the UI, not asserted.

### An LRU eviction test must distinguish LRU from FIFO

A plain sequential `get(0), get(1), get(2)` eviction test passes under FIFO too; add a hit-promotion case (re-hit an old key, then verify a later miss evicts the *other* key) to actually pin LRU recency semantics.

### Use `saturating_add`/`saturating_sub` for page-index arithmetic

(e.g. `center.saturating_add(radius)`) so debug builds don't panic on overflow.

### `rayon` is already transitive via `image`

adding it as a direct dependency pulls in no new third-party code; it just lets core `use rayon` directly.

### Thumbnails are a "hold ALL N pages" non-LRU set

the inverse of `ImageCache`'s sliding LRU. Generation is core's synchronous `generate_thumbnails` (rayon `par_iter`); the UI just launches it on a background thread so `open` returns immediately. Peak RAM ≈ rayon-pool-size full-res pages (one per worker, decoded-then-downscaled) — the same bound as prefetch.

### First cross-thread UI update convention (PR8a)

A rayon worker reaches the UI thread via `slint::invoke_from_event_loop`. Capture ONLY `Send` values into the closure: `slint::Weak` (Send+Sync), `Arc<AtomicUsize>`/`Arc<AtomicBool>`, `DecodedImage` (Send). `VecModel` (Rc) and `slint::Image` are NOT `Send` and never cross threads — re-fetch the model INSIDE the event-loop closure via `ui.get_thumbnails().as_any().downcast_ref::<VecModel<ThumbnailItem>>()`, and build the `slint::Image` there too (via `to_slint_image`, an O(pixels) copy done ONCE at generation, not per `refresh`).

### epoch + cancel DOUBLE-guard against superseded thumbnail generations

Re-opening a book (a) `cancel.store(true)` on the prior generation's flag (stops CPU promptly) AND (b) bumps an `AtomicUsize` epoch so any in-flight `invoke_from_event_loop` whose captured `my_epoch` mismatches the current epoch is dropped (prevents an old generation writing into the new model). Either guard alone is insufficient.

### Per-page thumbnail failure → distinct FAILED cell, not a silent/ambiguous placeholder

`generate_thumbnails` delivers the failure as `Err` (no panic). The worker logs `tracing::warn!(page, error)` (capturing the real `CoreError` WITHOUT crossing the thread boundary), then marshals a failed cell rendered distinctly (red ✕) so a permanent failure is visually separable from a still-loading gray cell (upholds the "view must match status" rule). `ThumbnailItem`'s `(loaded, failed)` pair is enforced through a private `enum ThumbCell { Loading, Loaded(slint::Image), Failed }` sum type (PR-B / issue #30): the single `fn thumbnail_item(page, cell) -> ThumbnailItem` chokepoint maps each variant to the correct boolean triple, eliminating the former three-site procedural enforcement; a `debug_assert!(!(loaded && failed))` inside `thumbnail_item` guards against any future hand-edit to the match arms (same `debug_assert` philosophy as `seq_index`). `ThumbCell::Loaded(slint::Image)` is `!Send`, so only the UI thread can construct it — the thread-boundary rule is type-enforced, not comment-only. The shared `invoke_from_event_loop` preamble (epoch-mismatch guard → `weak.upgrade()` → `get_thumbnails()` → downcast → row-count bound check) is centralized in `marshal_cell`, called by both the success and failure paths. `invoke_from_event_loop` errors are logged at `debug!` (not `let _`-swallowed) — the realistic trigger is an event-loop-gone race at teardown.

### The post-decode cancel check is tested deterministically, not racily

`generate_thumbnails` polls `cancelled` again AFTER decode / BEFORE callback; a single-page test source whose `read_bytes` flips the cancel flag as a side effect forces that second check to fire deterministically — avoiding the racy "flip the flag inside `on_ready`" approach, where other parallel tasks may have already passed the check. The background stream path itself (`invoke_from_event_loop`) stays coverage-EXEMPT (same as the cache rayon path); the synchronous `generate_thumbnails` carries the coverage.

### TouchArea click focus recovery for thumbnails

uses a Slint `public function focus-pages() { fs.focus(); }` called from Rust as `ui.invoke_focus_pages()` after a thumbnail click — the non-Button-click counterpart of the existing `clicked => fs.focus()` rule (a `TouchArea` click would otherwise leave the page `FocusScope` unfocused and silently kill keyboard navigation).

### `TouchArea.moved` fires only while pressed; any enabled `TouchArea` grabs the press (PR-S, slint 1.16.1)

`TouchArea.moved` fires ONLY while the pointer is pressed/grabbed — never on plain (unpressed) hover. And ANY enabled `TouchArea`, even one with no handlers, unconditionally GRABS the pointer press (`ForwardAndInterceptGrab` then `GrabMouse`), so layering one on top of another (e.g. an overlay over `PageView`) silently blocks the lower one's pan/drag — the lower `TouchArea`'s `pressed` never becomes true. To react to plain hover-movement WITHOUT stealing press/drag/scroll, do NOT add an overlay `TouchArea`: listen for `changed mouse-x` / `changed mouse-y` (or `has-hover`) INSIDE the existing `TouchArea` — `mouse-x`/`mouse-y`/`has-hover` update on every move, pressed or not. Concrete: PR-S reveals the auto-hiding chrome on mouse-move via `PageView`'s existing `TouchArea` (`changed mouse-x/mouse-y => reveal()`), after an initial overlay-`TouchArea` attempt broke pan and never fired on hover.

### Scrubber drag is preview-on-move, commit-on-release (PR-S)

During a scrubber drag, ONLY the preview popover + page-counter update: `preview(int)` pulls thumbnails from the existing `VecModel<ThumbnailItem>` and sets the counter text — it must NEVER call `jump_to`/`refresh`. The page body changes ONLY on knob release via `commit(int)` → `jump_to` → `refresh`. Keep all decode/navigation side effects on the commit path; preview is display-only and UI-thread-only (the `Rc`/`!Send` thumbnail model is never crossed).

### `if`-gated element ids are NOT reachable from the parent's `public function`s / `init` — gate with `visible:` when an id must be parent-reachable (PR-0b)

Slint scopes an id declared inside an `if`/`for` branch to a child the enclosing component cannot name, so a parent-level Rust-invoked seam like `focus-pages()`/`focus-carousel()` (or `init`) CANNOT `.focus()` an element under `if cond : Foo { ... }`. When a screen/region must be referenced by id from a parent function or `init`, gate it with `visible: <cond>` (keeps the id at root scope) instead of `if <cond>`. Trade-off: `visible:` keeps every branch instantiated (both screens live in the tree, toggled by visibility) — accepted here; focus is driven explicitly by the Rust seam functions on each transition. PR-0b's `ViewerWindow.slint` gates the Carousel (screen 0) and the Viewer body (screen 1) with `visible: root.screen == N` precisely so `focus-carousel()`/`focus-pages()` can reach `carousel`/`fs`.

### The cargo gates do NOT exercise Slint markup behavior — verify `.slint` logic against the spec by hand (PR-0b)

fmt/clippy/nextest cover Rust only; Slint key handlers, bindings, and visibility live in `.slint` markup that compiles via `build.rs` but has NO automated behavioral test (the project does not unit-test Slint visuals). After editing a `.slint` `FocusScope` key handler or property binding, explicitly check it against the spec — a missing key arm compiles and passes ALL three gates silently. Concrete PR-0b miss: the `Key.UpArrow -> nav("up")` arm (the entire point of the GoToLibrary feature) was initially omitted from the viewer `FocusScope` yet every gate stayed green; it was caught only by spec re-reading.

### Showing the thumbnail strip shrinks the `PageView` height, which auto-fires the existing `viewport-resized` wiring

no extra wiring needed for the `T` toggle. `SpreadMode::Auto` may re-resolve on that height change (accepted).

### Settings-dialog value setters are idempotent (same value → `false`, no-op) to absorb ComboBox self-fire

When Rust pushes a value into a bound `ComboBox.current-index`, `selected` can re-fire; the no-op-on-equal setters break the feedback loop. (Ties to the existing `jump_to` "did it move" convention.)

### Dialog cache/preload edits must reach `ViewerState` via `set_cache_config`, not just `Settings`

`ViewerState` seeds `cache_size`/`preload_pages` ONCE at `from_settings`; `set_source` builds the `ImageCache` from ViewerState's OWN fields, never re-reading live `Settings`. Updating only `Settings` makes the new value take effect on the NEXT LAUNCH; `set_cache_config` mirrors it so a book opened later THIS session uses it. Immediate rebuild of the CURRENT book's cache stays deferred.

### enum↔index helpers (`main.rs`) stay in lock-step with the ComboBox `model:` arrays

`*_to_index` uses an EXHAUSTIVE match (a new enum variant is a compile error); `index_to_*` defaults any out-of-range `i32` (Slint sends a raw int) to the FIRST variant. Round-trip + out-of-range-clamp are unit-tested.

### Modal overlays: `if root.show-X : Component` as the LAST children of the `Window` (last = front), sized `width/height: root.width/height`

The page `FocusScope` key handler guards `if (show-settings || show-guide) { return reject; }` so background nav keys don't drive the hidden viewer while a modal is up; closing an overlay calls `ui.invoke_focus_pages()` (the overlay counterpart of the Button `fs.focus()` rule; `focus-pages()` exists since PR8a). The "Settings…" button deliberately omits `fs.focus()` (the dialog needs focus). Dialogs dismiss via their own button only (no backdrop-click / Esc — flagged and intentionally deferred).

### Dialog save failures log `tracing::error!` (matching the other save sites, NOT `warn!`) AND surface to the status bar on close (`ui.set_status_text`)

A `tracing` line alone is invisible in a GUI run (`RUST_LOG` usually unset) — same rationale as surfacing the skipped count. The guide-dismiss save failure degrades gracefully (the guide simply re-shows next launch; `seen_guide` is also saved on exit) — intentional non-fatal.

### Runtime state is the SINGLE source of truth for the four display modes; `Settings` mirrors them ONLY via `reconcile_settings`, just before each save (PR-D / issue #32)

`ViewerState` owns `reading_direction`/`spread_mode`/`cover_mode`; `ViewportState` owns `fit_mode`. `reconcile_settings(&ViewerState, &ViewportState, &mut Settings)` (a pure fn in `main.rs`) copies those four into `Settings` immediately before EACH `save()` — exit, settings-dialog close, and the open-time save (now INSIDE the `if track_recent_files` gate in `open_and_present`, the only save on that path). Mode-mutation sites (D/R/C/`f` keys + the dialog setters) now ONLY mutate runtime state + `refresh`; the ~9 per-mutation `settings.borrow_mut().X = …` mirror lines are GONE, killing the "a new mutation site forgets to mirror → setting silently not persisted" bug class (neither types nor tests caught it before). The guide-dismiss save writes only `seen_guide` and intentionally SKIPS reconcile (not a runtime-mirrored field). EXCEPTION: `cache_size`/`preload_pages`/`track_recent_files` keep `Settings` as their source (one-way `Settings → ViewerState` via `set_cache_config` — see that bullet above); they are NOT reconciled back. `on_open_settings` reads the dialog's initial mode values from the RUNTIME (`state`/`viewport`), never `Settings`, so a lagging mirror can't make the dialog show a stale value.

### Key `Library` by the CANONICAL path, never the raw dialog path (PR-R)

Any code that keys into `Library` by path (`last_page`/`set_last_page`/`add`) MUST use the
**canonical** path form. `ViewerState::open_path` stores `path.canonicalize().unwrap_or(verbatim)`
in `open_file`, and `Library::add` applies the identical policy to the same input, so the keys
match. Resume/write-back therefore read the key from `state.open_file()`, NEVER the raw `path`
argument (which may carry `..`/symlinks/case differences). This is a SILENT-failure trap: a raw-path
lookup "succeeds" returning `last_page` = 0, so the bug presents as resuming at page 0 rather than an
error.

### Mirror the recents save-on-open convention when registering into another persisted store (PR-R)

When an open should register the item in a persisted store, follow the existing recents
`push_recent` + immediate `save()` on-open pattern so the stores stay consistent after a crash.
PR-R added `Library::add` + an immediate library `save()` on open precisely so a book can't appear
in recents but be missing from the shelf. Persistence-failure policy stays log-only
`tracing::error!`, consistent with the settings/recents save sites (a `tracing` line is invisible in
a GUI run, so genuinely user-facing failures additionally surface to the status bar — see the
dialog-save bullet).

### Borrow discipline for reconcile-before-save (PR-D)

Each `reconcile_settings(&state.borrow(), &viewport.borrow(), &mut settings.borrow_mut())` is ONE statement: the three temporaries (distinct RefCells) drop at the `;`, so the following fresh `settings.borrow().save()` cannot double-borrow. In `open_and_present`, bind `let opened = state.borrow_mut().open_folder(path);` FIRST (the `borrow_mut` drops at the `;`) so the `Ok` arm can read `&state.borrow()` in reconcile — a `borrow_mut` held across the `match` would double-borrow-panic. Inside `if s.track_recent_files`, reconcile REUSES the already-held `&mut s` (`s: RefMut<Settings>`) rather than taking a second `settings.borrow_mut()`. Pass `&mut s`, NOT `&mut *s` — `RefMut` deref-coerces to `&mut Settings` and clippy's `explicit_auto_deref` (`-D warnings`) rejects the explicit `*`. The `reconcile_settings` unit test pins BOTH directions: the four mirrored fields ARE written AND the non-mirrored fields (`cache_size`/`preload_pages`/`track_recent_files`/`seen_guide`) are left untouched (built via struct-update syntax to dodge `clippy::field_reassign_with_default`).

NUANCE (PR-R, `write_back_position`): to read MULTIPLE fields from one `RefCell` in a single expression, take ONE `let s = state.borrow();` block and read all fields from it (e.g. `position_to_write_back(s.open_file(), s.index())`) rather than `state.borrow()` twice in the same expression; let that `Ref` drop at the `;` before the later `borrow_mut()` (e.g. `set_last_page`) — and keep that `borrow_mut()` in its own statement, never held across a following `borrow()` (e.g. the subsequent `save()`).
