# Patterns & gotchas (learned the hard way)

This is the L2/L3 reference doc migrated from the CLAUDE.md "Patterns & gotchas" section.
An agent should read the relevant entry BEFORE editing the corresponding code area.

### Cross-crate mocking via a `testing` feature

`gashuu-core` gates `mockall::automock` on `PageSource` behind `[features] testing = ["dep:mockall"]`; `gashuu`'s dev-dependency enables it, so `ViewerState` tests use `MockPageSource` without pulling `mockall` into release builds.

### `#[allow(dead_code)]` on test-only accessors

In a *binary* crate `pub` is not a public API surface, so `-D warnings` flags an accessor used only by `#[cfg(test)]` code as dead; such `#[allow(dead_code)]` is intentional and documented in place. (PR8a's thumbnail-strip wiring now USES `ViewerState::page_count()`/`index()` at runtime, so they shed their `#[allow]` — the pattern still applies to any future test-only accessor.)

### When one rule must hold across the Slint↔Rust boundary, make ONE side authoritative — don't mirror it (PR-S → #71)

PR-S originally MIRRORED the scrubber knob-fraction → page mapping: a pure Rust twin (`scrub_fraction_to_page`) was the unit-tested spec, and `Scrubber.slint` carried an EXACT-mirror `drag-page` expression (same clamp, same round-half-up, RTL inverting the fraction before rounding). #71 deleted the Slint side: the scrubber now passes the RAW clamped knob fraction (a `float` in `[0,1]`) up via `preview(float)`/`commit(float)`, and `on_scrub_preview`/`on_scrub_commit` in `main.rs` call `scrub_fraction_to_page` to resolve the page. So Rust is the SINGLE LIVE source of that mapping (clamp, RTL inversion, round-half-up all live there) — it has a real runtime caller and is no longer `#[allow(dead_code)]` / test-only. THE LESSON: a mirrored rule drifts (the two sides silently diverge on the next edit to one of them). When a single rule must hold on both sides of the Slint↔Rust boundary, make ONE side authoritative — let Slint pass the raw inputs across the boundary and compute the rule once in Rust, rather than re-deriving it in markup the cargo gates cannot test.

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

`from_json` normalizes after deserialize: `cache_size` is floored to 1 via `Settings::cache_config()`, which constructs a `CacheConfig` whose `new` enforces `capacity >= 1` (the single source of truth for that floor — `ImageCache::new` no longer clamps; it consumes a `CacheConfig` whose capacity is already guaranteed `>= 1`, so the `NonZeroUsize::new(...).unwrap()` inside it is provably safe). `recent_files.truncate(MAX_RECENT_FILES)` prevents an over-long hand-edited list from persisting forever via exit-save. **`preload_pages` is deliberately NOT clamped** — 0 is a valid "prefetch disabled" radius and not coerced downstream, so clamping it would silently override a legitimate user choice.

### Value objects own their invariants; pass the whole object, not its fields

When a primitive carries an invariant (e.g. an LRU `capacity` that must be `>= 1`), don't
re-assert it with scattered `.max(1)` at each layer. Wrap it in a small immutable value object
that enforces the invariant in its constructor, so an invalid value cannot exist downstream.
`CacheConfig` (PR59, `cache_config.rs`) is the canonical example: `new(capacity, radius)` clamps
`capacity` to `>= 1` once, exposes read-only `capacity()`/`radius()`, and is `Copy`. Because every
`CacheConfig` is valid, `ImageCache::new`'s `NonZeroUsize::new(config.capacity()).unwrap()` is
provably infallible -- the clamp it used to do internally is gone.

Three hard-won rules for such a type:

1. **Do NOT `#[derive(Deserialize)]` on it.** serde populates the private fields directly,
   bypassing the constructor and its clamp -- re-opening the invalid-state hole a corrupt or
   hand-edited file could exploit. Keep the value object serde-free; persist the *raw* primitives
   on a separate struct (`Settings { cache_size, preload_pages }`) and expose a getter
   (`Settings::cache_config()`) that builds the validated object on read. That getter is the
   canonical conversion but not the *only* construction site (the settings dialog rebuilds one
   live), so don't over-claim "the single point" in its doc.
2. **Single-source the floor.** If you also keep a storage-hygiene normalization on the load path,
   route it through the value object instead of repeating the literal:
   `settings.cache_size = settings.cache_config().capacity();` -- now the `>= 1` floor is defined in
   exactly one place (`CacheConfig::new`). See the read-path-invariant entry above for why the
   stored field is still normalized at all.
3. **Pass the whole object to constructors, not its fields.** `ImageCache::new(source, config)`
   beats `ImageCache::new(source, capacity, radius)`: two same-typed `usize` args are a silent
   transposition footgun (swap capacity/radius and it still compiles), whereas a single
   `CacheConfig` cannot be mis-ordered. Keep the two-field mapping
   (`CacheConfig::new(cache_size, preload_pages)`) in one tested place so the only transpose risk
   is also the only thing under test.

### Cohesion-wrapper value object: bundle args-that-always-travel-together, delegate to free fns

A second, distinct value-object flavor — different from the invariant-owning kind above. Use it
when several same-typed arguments are always passed together to a cluster of pure free functions,
and the positional ordering is a silent transposition footgun (the compiler cannot distinguish them).
`SpreadContext` (PR66, `spread.rs`) is the canonical example: it bundles
`(total: Option<usize>, layout: SpreadLayout, cover: CoverMode)` into a `Copy` struct and exposes
`.spread_at(i)`, `.next(i)`, `.prev(i)`, `.normalize(i)` — each delegating directly to the
corresponding free fn (`spread_at`, `next_leading`, `prev_leading`, `normalize_leading`). A private
`ViewerState::spread_ctx()` assembles it once; the six call sites switch from three positional args
to a single named receiver, making the wrong argument order a compile error.

Four hard rules for this flavor:

1. **Delegate; do NOT rewrite.** The free fns remain the single source of truth. The wrapper
   methods are one-liners that forward to them — do not inline or copy the logic into the struct.

2. **Stay additive; do NOT tighten invariants or widen the API.** A cohesion wrapper's contract is
   "same behavior, named home." Do not add `debug_assert!`s on fields, do not add field accessors
   that expose internal state, and do not move resolution logic into it (e.g. `Auto → SpreadLayout`
   resolution stays outside the wrapper). Doing so changes observable behavior (a `debug_assert`
   panics in debug/test builds; a new accessor widens the public surface) — that is a separate PR.
   In PR66 a reviewer proposed `debug_assert!(total > 0)`; it was deliberately DECLINED because the
   free fns intentionally tolerate `total == 0` defensively and the issue mandated "no behavior
   change."

3. **Test both enum variants in every delegation test.** A delegation test that only exercises
   `SpreadLayout::Single` can silently pass even with a transposed-field copy-paste error, because
   `Single`-layout `normalize_leading` is an identity function (transposed args still return the
   same value). Test `Single` AND `Double` for each method; a `Double` call exposes a wrong-field
   bug that `Single` masks.

4. **Constructor is trivial (no clamping, no validation) — no `#[derive(Deserialize)]` concern.**
   Because there is no enforced invariant, there is no serde bypass hazard; the no-Deserialize rule
   in the invariant-owning section above does NOT apply here. The struct is `Copy` and all fields
   may be public or private — choose whichever keeps the construction site readable.

### Use-case object (collaborator-owning): bundle shared `Rc<RefCell<…>>` as fields, expose `run`

A THIRD value-object flavor — neither the invariant-owner (CacheConfig #59, above) nor the
cohesion-wrapper (SpreadContext #66, above). Use it when a free fn coordinates a multi-step use case
while THREADING many shared `Rc<RefCell<…>>` collaborators — the `#[allow(clippy::too_many_arguments)]`
smell. Bundle the collaborators as private FIELDS of a `pub(crate) struct XUseCase`, construct it once,
and expose `run(&self, …per-call args…)` carrying the moved body. `OpenBookUseCase` (PR67, `app.rs`)
is the canonical example: it owns the six open-path collaborators
(`state`/`settings`/`viewport`: `Rc<RefCell<_>>`, `library`: `Rc<RefCell<Library>>`,
`thumbs`: `Rc<ThumbnailController>`, `covers`: `Rc<CoverController>`) and exposes
`run(&self, ui: &ViewerWindow, path: &Path, skipped_detail: &str)`. It replaces the former
nine-argument `open_and_present` free fn (which carried `#[allow(clippy::too_many_arguments)]`).

**WHY:** removes the nine-arg signature AND the `#[allow]`; collapses the per-closure `Rc::clone`
ceremony — the three open handlers used to clone all six collaborators each; now each does ONE
`Rc::clone(&open_book)` then `open_book.run(ui, path, detail)`. It gives the use case a NAME and a
single reviewable home. It stays in the UI crate because it touches Slint (status text, carousel
rebuild, thumbnail launch) — `gashuu-core` stays headless.

**CONTRAST with the two flavors above:** the invariant-owner owns a CLAMPED primitive in an
immutable ctor; the cohesion-wrapper is a `Copy` bundle that DELEGATES to surviving free fns with no
behaviour change. The use-case object instead OWNS shared mutable `Rc` handles and IS the moved body
— there are no peer free fns to delegate to. Its "invariant" is collaborator-completeness, not a
clamped value, so a trivial infallible ctor is correct and there is nothing to enforce (and no
`#[derive(Deserialize)]` hazard, same as the cohesion-wrapper).

**The verbatim-move-with-field-aliases harness** (the load-bearing how-to):

1. **Alias the fields to locals at the top of `run`** (`let state = &self.state;` for each field) so
   the moved body reads BYTE-IDENTICAL to the former parameters. The body's `&Rc<T>` resolves to the
   former `&T` transparently through `Deref` for method calls (e.g. `thumbs.start(...)` works on
   `&Rc<ThumbnailController>` exactly as on the old `&ThumbnailController`), so NO statement in the
   body changes. This minimises the diff/review surface and preserves the dense borrow-discipline
   comments verbatim.

2. **Extract at least one PURE inline decision for headless unit tests.** `run` itself is Slint-coupled,
   so the cargo gates never exercise it. PR67 lifted the status-compose decision into
   `pub(crate) fn status_notices(skipped, skipped_detail, settings_save, library_save) -> Vec<String>`
   — pure, so the "skipped, then settings-save failure, then library-save failure" order is unit-tested
   without a UI (mirrors the `position_to_write_back` precedent). The `run` body just iterates
   `status_notices(...)` and appends each onto the current status; the old `append_status` closure is
   gone.

3. **Preserve borrow discipline EXACTLY** (the moved body carries the same RefCell-drop choreography):
   single-statement `reconcile_settings(&state.borrow(), &viewport.borrow(), &mut s)`;
   `canonical = state.borrow().open_file()…` whose `Ref` drops at its `;`; `register_opened` →
   `jump_to` kept as separate statements on distinct `RefCell`s; refresh-BEFORE status-compose
   ordering; the `count_changed`-gated carousel rebuild.

4. **Slint gotcha:** the new submodule needed `use slint::ComponentHandle;` for `ui.as_weak()`. A
   submodule does NOT inherit the crate-root `include_modules!` trait scope that `main.rs` enjoys, so
   trait methods on generated Slint types must be brought into scope explicitly.

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

`ViewerState` keeps only `index` (= current spread's leading page) + the modes; the spread is recomputed each call via `spread_at` (avoids dual-source drift). Invariant: `index` is ALWAYS a valid spread-start — reset to 0 on `set_source`, mutated only via `next_/prev_leading`, and re-anchored by `normalize_leading` after a `spread_mode`/`cover_mode` toggle so the visible page stays on screen. `reading_direction` toggles do NOT normalize (pairing is direction-agnostic). In practice `ViewerState` assembles the `(total, layout, cover)` triple once via `spread_ctx()` (a `SpreadContext`) and calls `.next()`/`.prev()`/`.normalize()` on it; the free functions remain the source of truth.

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

This epoch+cancel + Send/!Send streaming shape is the REUSABLE HARNESS for "stream a background-decoded image into a Slint model row". It now has TWO consumers: `ThumbnailController` (`thumbnail_strip.rs`) and PR-V's `CoverController` (`cover_loader.rs`, one cover per Library book). A third consumer should mirror it the same way: pre-open cancel check → post-generate cancel re-check → epoch guard inside the marshal closure; only `Send` values cross into the worker; the `Rc` `VecModel` and `slint::Image` are fetched/built INSIDE the closure; the non-`Clone` `ThumbnailCache` is reconstructed on the worker. The 3-statement cancel-rotation borrow discipline (store `true` → install a fresh `Arc` → clone it out) is factored into a private `fn rotate_cancel(&self) -> Arc<AtomicBool>` kept IDENTICAL in both controllers — a third consumer should carry the same-shape helper rather than re-deriving the discipline.

### Per-page thumbnail failure → distinct FAILED cell, not a silent/ambiguous placeholder

`generate_thumbnails` delivers the failure as `Err` (no panic). The worker logs `tracing::warn!(page, error)` (capturing the real `CoreError` WITHOUT crossing the thread boundary), then marshals a failed cell rendered distinctly (red ✕) so a permanent failure is visually separable from a still-loading gray cell (upholds the "view must match status" rule). `ThumbnailItem`'s `(loaded, failed)` pair is enforced through a private `enum ThumbCell { Loading, Loaded(slint::Image), Failed }` sum type (PR-B / issue #30): the single `fn thumbnail_item(page, cell) -> ThumbnailItem` chokepoint maps each variant to the correct boolean triple, eliminating the former three-site procedural enforcement; a `debug_assert!(!(loaded && failed))` inside `thumbnail_item` guards against any future hand-edit to the match arms (same `debug_assert` philosophy as `seq_index`). `ThumbCell::Loaded(slint::Image)` is `!Send`, so only the UI thread can construct it — the thread-boundary rule is type-enforced, not comment-only. The shared `invoke_from_event_loop` preamble (epoch-mismatch guard → `weak.upgrade()` → `get_thumbnails()` → downcast → row-count bound check) is centralized in `marshal_cell`, called by both the success and failure paths. `invoke_from_event_loop` errors are logged at `debug!` (not `let _`-swallowed) — the realistic trigger is an event-loop-gone race at teardown.

### The post-decode cancel check is tested deterministically, not racily

`generate_thumbnails` polls `cancelled` again AFTER decode / BEFORE callback; a single-page test source whose `read_bytes` flips the cancel flag as a side effect forces that second check to fire deterministically — avoiding the racy "flip the flag inside `on_ready`" approach, where other parallel tasks may have already passed the check. The background stream path itself (`invoke_from_event_loop`) stays coverage-EXEMPT (same as the cache rayon path); the synchronous `generate_thumbnails` carries the coverage.

### TouchArea click focus recovery for thumbnails

uses a Slint `public function focus-pages() { fs.focus(); }` called from Rust as `ui.invoke_focus_pages()` after a thumbnail click — the non-Button-click counterpart of the existing `clicked => fs.focus()` rule (a `TouchArea` click would otherwise leave the page `FocusScope` unfocused and silently kill keyboard navigation).

### `TouchArea.moved` fires only while pressed; any enabled `TouchArea` grabs the press (PR-S, slint 1.16.1)

`TouchArea.moved` fires ONLY while the pointer is pressed/grabbed — never on plain (unpressed) hover. And ANY enabled `TouchArea`, even one with no handlers, unconditionally GRABS the pointer press (`ForwardAndInterceptGrab` then `GrabMouse`), so layering one on top of another (e.g. an overlay over `PageView`) silently blocks the lower one's pan/drag — the lower `TouchArea`'s `pressed` never becomes true. To react to plain hover-movement WITHOUT stealing press/drag/scroll, do NOT add an overlay `TouchArea`: listen for `changed mouse-x` / `changed mouse-y` (or `has-hover`) INSIDE the existing `TouchArea` — `mouse-x`/`mouse-y`/`has-hover` update on every move, pressed or not. Concrete: PR-S reveals the auto-hiding chrome on mouse-move via `PageView`'s existing `TouchArea` (`changed mouse-x/mouse-y => reveal()`), after an initial overlay-`TouchArea` attempt broke pan and never fired on hover.

### Scrubber drag is preview-on-move, commit-on-release (PR-S)

During a scrubber drag, ONLY the preview popover + page-counter update: `preview(float)` resolves the raw fraction to a page (via `scrub_fraction_to_page`), pulls thumbnails from the existing `VecModel<ThumbnailItem>`, and sets the counter text — it must NEVER call `jump_to`/`refresh`. The page body changes ONLY on knob release via `commit(float)` → `jump_to` → `refresh`. Keep all decode/navigation side effects on the commit path; preview is display-only and UI-thread-only (the `Rc`/`!Send` thumbnail model is never crossed). (Both callbacks carry the RAW clamped fraction, not a page index — see the authoritative-side boundary entry above.)

### Only the INSTANTIATED root window's surface is reachable from Rust — re-expose child properties/callbacks on the root (PR-L)

Slint's generated Rust API exposes ONLY the properties/callbacks/`public function`s declared on the window component `main.rs` instantiates (`ViewerWindow`). A child component's internal `in property`/callback (e.g. `Carousel.items`, `Carousel.add-files()`) is INVISIBLE to Rust — there is no generated accessor for it. To wire a child property/handler from Rust, declare a twin on the ROOT and bind/forward it to the child: `ViewerWindow` exposes `in property <[CarouselItem]> carousel-items` bound by `items: root.carousel-items;`, and root `add-files()`/`add-folder()` callbacks forwarded into the `Carousel`. Generated name mapping: kebab→snake_case, `set_<prop>`/`get_<prop>`, `on_<callback>`, `invoke_<public function>` (e.g. `set_carousel_items`, `on_add_files`, `invoke_focus_carousel`). When adding a new Rust-driven property/handler, put it on the root window first — not only on the child.

### A callback SIGNATURE change ripples to THREE places, not one (#71)

A child-component callback whose type changes (e.g. the scrubber's `preview`/`commit` going from `int` to `float` when #71 moved fraction→page rounding into Rust) must be edited in all THREE: (1) the child component `.slint` declares the callback; (2) the `ViewerWindow` root TWIN callback `.slint` re-declares + forwards it (Rust binds only the root window surface — see the entry above, so the child's callback alone is invisible to Rust); and (3) the Rust closure(s) (`on_scrub_preview`/`on_scrub_commit`) that receive the new type. Miss any one and it either won't compile (Rust closure type mismatch) or won't wire (an unforwarded root twin). Search for both the child name and the kebab→snake twin name when changing a callback's type.

### `if`-gated element ids are NOT reachable from the parent's `public function`s / `init` — gate with `visible:` when an id must be parent-reachable (PR-0b)

Slint scopes an id declared inside an `if`/`for` branch to a child the enclosing component cannot name, so a parent-level Rust-invoked seam like `focus-pages()`/`focus-carousel()` (or `init`) CANNOT `.focus()` an element under `if cond : Foo { ... }`. When a screen/region must be referenced by id from a parent function or `init`, gate it with `visible: <cond>` (keeps the id at root scope) instead of `if <cond>`. Trade-off: `visible:` keeps every branch instantiated (both screens live in the tree, toggled by visibility) — accepted here; focus is driven explicitly by the Rust seam functions on each transition. PR-0b's `ViewerWindow.slint` gates the Carousel (screen 0) and the Viewer body (screen 1) with `visible: root.screen == N` precisely so `focus-carousel()`/`focus-pages()` can reach `carousel`/`fs`.

### `root` is the COMPONENT root; `parent` is the IMMEDIATE enclosing element only — count the nesting when reading a `for`-item property (PR-C, slint 1.16.1)

`root.<name>` resolves to the component's root element, NOT to a `property` declared on a nested element such as a `Repeater` `for`-item `Rectangle`. To read a property declared on the for-item element from a DIRECT child, use `parent.<name>`. (Bug: a `private property <length> row-cy` on the cover-flow row `Rectangle` was wrongly read as `root.row-cy`; the fix is `parent.row-cy`.) And `parent` climbs exactly ONE level: a property on the for-item `Rectangle` (e.g. a per-item `focused: bool`) is NOT reachable from a GRANDCHILD via `parent.<name>` — at that depth `parent` is the intervening element. (Bug: a per-`Image` `colorize: parent.focused ? …` failed because `parent` there was the inner cover `Rectangle`, which has no `focused` — that lives two levels up on the for-row `Rectangle`; the recede cue was carried by the for-row's `opacity` instead.) Neither error is caught by the cargo gates (see the spec-by-hand entry below); both compile-fail only at `build.rs` Slint compile or render wrong silently.

### Slint 1.16.1 accepted syntax + limitations (verified at impl time, PR-C)

Confirmed ACCEPTED by the pinned Slint 1.16.1 (future work need not re-verify): `Math.clamp(x, lo, hi)`, `overflow: elide` on `Text`, `color.with-alpha(f)`, 8-digit `#rrggbbaa` hex, and `@linear-gradient(deg, stop% … )`. NOT supported: per-`Repeater`-item z-order is not settable in Slint 1.x — layer a focused item via opacity/size/accent-ring, not z; no `line-height` on `Text` in Slint 1.x — DESIGN's per-role `lineHeight` cannot be expressed and must not be faked (space elements apart instead). A shared PRIVATE (non-exported) sub-component (`component Foo inherits Rectangle { in property … }`) cleanly de-duplicates repeated markup WITHIN one file WITHOUT touching any exported struct/component contract, and works both absolutely positioned and as a layout child (PR-C used a file-private `ProgressBar` for the per-cover and focused-meta bars in `Carousel.slint`). When the same markup must be reused ACROSS files, promote it to an `export`ed component under `ui/components/` instead (#71 did exactly this — `ProgressBar` is now `components/ProgressBar.slint`, imported by `Carousel.slint`); keep the file-private form only when the reuse is confined to one file.

### The cargo gates do NOT exercise Slint markup behavior — verify `.slint` logic against the spec by hand (PR-0b)

fmt/clippy/nextest cover Rust only; Slint key handlers, bindings, and visibility live in `.slint` markup that compiles via `build.rs` but has NO automated behavioral test (the project does not unit-test Slint visuals). After editing a `.slint` `FocusScope` key handler or property binding, explicitly check it against the spec — a missing key arm compiles and passes ALL three gates silently. Concrete PR-0b miss: the `Key.UpArrow -> nav("up")` arm (the entire point of the GoToLibrary feature) was initially omitted from the viewer `FocusScope` yet every gate stayed green; it was caught only by spec re-reading.

### Slint compiles only what is REACHABLE from the entry file — create-and-consume are verified together (#71)

`build.rs` compiles the single entry `ui/ViewerWindow.slint`; `import` statements cascade to pull in only the files reachable from it (which is why adding the new `ui/components/` atoms/molecules needed NO `build.rs` change). The flip side: a component under `ui/components/` is NOT compiled until some reachable file imports it, so a standalone component's syntax errors surface ONLY on its first consumption. Treat create-and-consume as one step — adding a component AND wiring its first consumer in the same change is what actually exercises the new file; an unimported component can sit broken with every gate green.

### A component that `inherits Rectangle` has NO intrinsic layout size (#71)

A `component Foo inherits Rectangle` carries no preferred/minimum size, so dropping it into a `HorizontalLayout`/`VerticalLayout` gives it zero height (or zero width) unless the consumer supplies `min-height`/`min-width` or a stretch. The shared `ThumbnailCell` (which `inherits Rectangle`) needs explicit `horizontal-stretch: 1` + `min-height` at each layout call site (the scrubber preview popover and the strip) to occupy the area the old inline `Image` did — the `Image` had an intrinsic size the bare `Rectangle` lacks.

### Showing the thumbnail strip shrinks the `PageView` height, which auto-fires the existing `viewport-resized` wiring

no extra wiring needed for the `T` toggle. `SpreadMode::Auto` may re-resolve on that height change (accepted).

### Settings-dialog value setters are idempotent (same value → `false`, no-op) to absorb ComboBox self-fire

When Rust pushes a value into a bound `ComboBox.current-index`, `selected` can re-fire; the no-op-on-equal setters break the feedback loop. (Ties to the existing `jump_to` "did it move" convention.)

### Dialog cache/preload edits must reach `ViewerState` via `set_cache_config`, not just `Settings`

`ViewerState` seeds `cache_size`/`preload_pages` ONCE at `from_settings`; `set_source` builds the `ImageCache` from ViewerState's OWN fields, never re-reading live `Settings`. Updating only `Settings` makes the new value take effect on the NEXT LAUNCH; `set_cache_config` mirrors it so a book opened later THIS session uses it. Immediate rebuild of the CURRENT book's cache stays deferred.

### enum↔index helpers (`enum_adapters.rs`) stay in lock-step with the ComboBox `model:` arrays

`*_to_index` uses an EXHAUSTIVE match (a new enum variant is a compile error); `index_to_*` defaults any out-of-range `i32` (Slint sends a raw int) to the FIRST variant. Round-trip + out-of-range-clamp are unit-tested.

### Modal overlays: `if root.show-X : Component` as the LAST children of the `Window` (last = front), sized `width/height: root.width/height`

The page `FocusScope` key handler guards `if (show-settings || show-guide) { return reject; }` so background nav keys don't drive the hidden viewer while a modal is up; closing an overlay calls `ui.invoke_focus_pages()` (the overlay counterpart of the Button `fs.focus()` rule; `focus-pages()` exists since PR8a). The "Settings…" button deliberately omits `fs.focus()` (the dialog needs focus). Dialogs dismiss via their own button only (no backdrop-click / Esc — flagged and intentionally deferred).

### Dialog save failures log `tracing::error!` (matching the other save sites, NOT `warn!`) AND surface to the status bar on close (`ui.set_status_text`)

A `tracing` line alone is invisible in a GUI run (`RUST_LOG` usually unset) — same rationale as surfacing the skipped count. The guide-dismiss save failure degrades gracefully (the guide simply re-shows next launch; `seen_guide` is also saved on exit) — intentional non-fatal.

Routing the outcome to a status property is only half the fix: a bound, VISIBLE widget must exist on the screen where the action RUNS (PR-L). The shared `status-text` is shown by a Viewer-screen `Text` gated `visible: screen == 1`; a Library-screen action (Add Files/Folder on screen 0) would set the property silently with nothing on screen. PR-L therefore mounted a second `status-text`-bound `Text` gated `visible: screen == 0`. Rule: route user-facing outcomes to a widget visible in the CURRENT screen, not just to any bound property.

### `refresh()` OVERWRITES `status-text` — surface notices AFTER it, and COMPOSE (append) when several can co-occur (PR-La)

`refresh()` pushes the base spread/status string into `status-text`, so any load/save error notice set BEFORE it is silently clobbered. Set such notices AFTER `refresh()` (the startup load-failure notice is set after the *initial* refresh; the open-path save-failure notices after the open-path refresh). When more than one notice can fire from a single action — on the open path: skipped entries + settings-save failure + library/page-count-save failure — COMPOSE them by APPENDING to the current status (`{base} \u{2014} {detail}`, em-dash), never replacing, so an earlier notice isn't lost. `app::OpenBookUseCase::run` (the former `open_and_present`, see the use-case-object bullet above) decides WHICH notices appear via the pure `status_notices(...)` fn and iterates its `Vec<String>`, appending each onto the current status (the old single `append_status` closure is gone); the save outcomes are captured into locals (`settings_save`, `library_save`) BEFORE `refresh` and surfaced after it, in a fixed order (skipped, then settings, then library). (Extends the PR-L "route to a visible widget" bullet above with the refresh-clobber + compose-don't-replace angle.)

### Runtime state is the SINGLE source of truth for the four display modes; `Settings` mirrors them ONLY via `reconcile_settings`, just before each save (PR-D / issue #32)

`ViewerState` owns `reading_direction`/`spread_mode`/`cover_mode`; `ViewportState` owns `fit_mode`. `reconcile_settings(&ViewerState, &ViewportState, &mut Settings)` (a pure fn in `main.rs`) copies those four into `Settings` immediately before EACH `save()` — exit, settings-dialog close, and the open-time save (INSIDE the `if track_recent_files` gate in `app::OpenBookUseCase::run`, the only save on that path). Mode-mutation sites (D/R/C/`f` keys + the dialog setters) now ONLY mutate runtime state + `refresh`; the ~9 per-mutation `settings.borrow_mut().X = …` mirror lines are GONE, killing the "a new mutation site forgets to mirror → setting silently not persisted" bug class (neither types nor tests caught it before). The guide-dismiss save writes only `seen_guide` and intentionally SKIPS reconcile (not a runtime-mirrored field). EXCEPTION: `cache_size`/`preload_pages`/`track_recent_files` keep `Settings` as their source (one-way `Settings → ViewerState` via `set_cache_config` — see that bullet above); they are NOT reconciled back. `on_open_settings` reads the dialog's initial mode values from the RUNTIME (`state`/`viewport`), never `Settings`, so a lagging mirror can't make the dialog show a stale value.

### Key `Library` by the CANONICAL path, never the raw dialog path (PR-R)

Any code that keys into `Library` by path (`last_page`/`set_last_page`/`add`) MUST use the
**canonical** path form. `ViewerState::open_path` stores `path.canonicalize().unwrap_or(verbatim)`
in `open_file`, and `Library::add` applies the identical policy to the same input, so the keys
match. Resume/write-back therefore read the key from `state.open_file()`, NEVER the raw `path`
argument (which may carry `..`/symlinks/case differences). This is a SILENT-failure trap: a raw-path
lookup "succeeds" returning `last_page` = 0, so the bug presents as resuming at page 0 rather than an
error.

### Derive UI state from the authoritative POST-OP state, not the request input (#71)

`OpenBookUseCase::run` returns `()` and bails on `Err` (via `open_path`'s `?` before `set_source`), so a FAILED open does not signal failure to the caller. The viewer title bar therefore derives the current book name from `ViewerState::open_file()` AFTER `run` returns — the canonical path set ONLY on a successful open — NOT from the dialog path passed into `run`. Reading post-op state makes a wrong title structurally impossible: a failed open leaves `open_file()` unchanged (empty on boot, or the still-open prior book), so the title can never show a book that did not open, and it uses the same canonical key the library write-back uses. The general rule: when a multi-step op can fail silently (returns `()`, mutates shared state on success only), drive dependent UI from the op's resulting state, not from the inputs you handed it.

### Mirror the recents save-on-open convention when registering into another persisted store (PR-R)

When an open should register the item in a persisted store, follow the existing recents
`push_recent` + immediate `save()` on-open pattern so the stores stay consistent after a crash.
PR-R added `Library::add` + an immediate library `save()` on open precisely so a book can't appear
in recents but be missing from the shelf. Persistence-failure policy stays log-only
`tracing::error!`, consistent with the settings/recents save sites (a `tracing` line is invisible in
a GUI run, so genuinely user-facing failures additionally surface to the status bar — see the
dialog-save bullet).

### Borrow discipline for reconcile-before-save (PR-D)

Each `reconcile_settings(&state.borrow(), &viewport.borrow(), &mut settings.borrow_mut())` is ONE statement: the three temporaries (distinct RefCells) drop at the `;`, so the following fresh `settings.borrow().save()` cannot double-borrow. In `app::OpenBookUseCase::run`, bind `let opened = state.borrow_mut().open_folder(path);` FIRST (the `borrow_mut` drops at the `;`) so the `Ok` arm can read `&state.borrow()` in reconcile — a `borrow_mut` held across the `match` would double-borrow-panic. Inside `if s.track_recent_files`, reconcile REUSES the already-held `&mut s` (`s: RefMut<Settings>`) rather than taking a second `settings.borrow_mut()`. Pass `&mut s`, NOT `&mut *s` — `RefMut` deref-coerces to `&mut Settings` and clippy's `explicit_auto_deref` (`-D warnings`) rejects the explicit `*`. The `reconcile_settings` unit test pins BOTH directions: the four mirrored fields ARE written AND the non-mirrored fields (`cache_size`/`preload_pages`/`track_recent_files`/`seen_guide`) are left untouched (built via struct-update syntax to dodge `clippy::field_reassign_with_default`).

NUANCE (PR-R, `write_back_position`): to read MULTIPLE fields from one `RefCell` in a single expression, take ONE `let s = state.borrow();` block and read all fields from it (e.g. `position_to_write_back(s.open_file(), s.index())`) rather than `state.borrow()` twice in the same expression; let that `Ref` drop at the `;` before the later `borrow_mut()` (e.g. `set_last_page`) — and keep that `borrow_mut()` in its own statement, never held across a following `borrow()` (e.g. the subsequent `save()`).

### Persistent cache keys must use a version-stable hash, not `DefaultHasher` (PR-T)

`std::hash::DefaultHasher` (and the `Hash` derive feeding it) is documented as NOT stable across Rust versions or platforms. Using it to name on-disk cache entries means a routine toolchain bump silently changes every key, orphaning the whole cache — no error, just a cold cache and wasted regeneration. `thumbnail_cache::cache_key` therefore hashes with a hand-written FNV-1a 64-bit (`FNV_OFFSET_BASIS`/`FNV_PRIME`, xor-then-multiply) over the path's `OsStr` bytes + `mtime.to_le_bytes()` + `max_side.to_le_bytes()`, formatted as 16 hex chars. FNV-1a is a fixed algorithm, so identical inputs map to the same filename across builds. Path bytes are platform-native (`OsStr::as_encoded_bytes`), which is fine because the cache is per-machine. Rule: any hash that NAMES a persisted artifact must come from a fixed algorithm, never `DefaultHasher`.

### Durable cache writes are temp-file-then-rename for reader atomicity (PR-T)

`ThumbnailCache::put` encodes to memory, writes `<dir>/.{key}.tmp`, then `std::fs::rename`s it onto `<dir>/{key}.png`. The rename is atomic on POSIX, so a concurrent `get` (e.g. PR-V's background rayon fill racing a read) never observes a half-written PNG — it sees either the old file or the complete new one. This guarantees READER atomicity only. Concurrent same-key WRITERS share the deterministic `.{key}.tmp` path and could clobber each other or orphan a `.tmp` on a failed rename; that is intentionally deferred. PR-V (cover generation) has now landed and does NOT trigger this: each book's cover key is distinct (path + mtime + max_side), and a `get` hit skips the worker entirely, so no two in-flight `put`s ever share a key. The deferral therefore still holds — the risk is simply not exercised. If a future PR DOES add parallel same-key writes (e.g. two threads regenerating one book's cover), switch to a unique temp name (pid + counter) plus best-effort cleanup then. Correspondingly, `get` treats every missing/unreadable/corrupt file as `None` (a cache miss), never an error, and never panics.

### Add a persisted core field with `#[serde(default)]` — bump `LIBRARY_VERSION` / change `migrate` ONLY when it can't be a defaulted field (PR-La)

`Book::page_count` was added as a `#[serde(default)]` field, so an older `library.json` (written before the field existed) still deserializes unchanged — the missing field defaults to `0`. NO `LIBRARY_VERSION` bump and NO `migrate` change was needed (same mechanism as `Book::last_page`, and as `Settings`' forward-compat fields). Reserve a version bump + `migrate` step for a change that a defaulted field cannot express (a renamed/removed/semantically-reshaped field). A schema test asserts the new field is EMITTED (`to_json`'s `page_count` is present) so it can't silently drop, plus a round-trip test that an old-shape `Book` JSON (no `page_count`) deserializes to the `0` sentinel.

### `0 = unknown` for a `usize` count — keep it in STORAGE, surface it as `Option`/`NonZeroUsize`, and still beware the legit-zero-pages trap (PR-La → #65)

The not-yet-known count was originally a bare `0` sentinel on `Book::page_count`, exposed through a public `page_count() -> usize` accessor and a `debug_assert!(count > 0)` setter. #65 hid that sentinel behind the type system: the STORAGE field `Book.page_count: usize` is UNCHANGED (`#[serde(default)]`, `0` still written to disk for an unknown/old file, `LIBRARY_VERSION` still 1, byte-compat preserved), but the PUBLIC surface is now `Option`/`NonZeroUsize` — `Book::page_count_opt() -> Option<usize>` (maps stored `0 → None`; the old `page_count() -> usize` is gone) and `Library::set_page_count(_, NonZeroUsize)`.

THE TRAP is still real: `ViewerState::open_path` returns `Ok(())` even for a source that opens with ZERO pages (empty folder, or an archive whose every entry was zip-slip/oversized-skipped), so a *successful* open can legitimately carry a count of `0`. The fix is no longer an `if n > 0` caller guard — it is converting AT THE BOUNDARY: `NonZeroUsize::new(page_count)` maps that legit zero to `None`, and `register_opened(Option<NonZeroUsize>)` simply skips the back-fill for `None`. The reader side flows from the same place: `Book::page_count_opt()` yields `None`, and `ReadingProgress::fraction` collapses an unknown total to `0.0`, so a never-opened book reads as unread.

`Book::last_page` is a DIFFERENT case, not a removed sentinel: there `0` means "first page / never advanced" — a real, valid value — so it stays a plain `usize` with no `Option` wrapper.

### Prefer a type over scattered runtime guards when the invariant is expressible (#65 supersedes the PR-60 two-layer pattern)

PR-60 enforced "page count > 0" as a two-layer RUNTIME pattern: in core (no `tracing` — core stays logging-free) `Library::set_page_count` carried `debug_assert!(count > 0)`; at the UI call site the caller short-circuited with `if page_count > 0 { library.set_page_count(…) }` so a legit zero-page open never reached the assert; and an "unreachable" UI branch got a `tracing::warn!` (UI-only, since `tracing` is forbidden in core) to make a future invariant break debuggable rather than silently wrong.

#65 LIFTED that invariant into the TYPE SYSTEM and thereby DISSOLVED all three pieces. `set_page_count(_, NonZeroUsize)` and `register_opened(_, Option<NonZeroUsize>)` make `0` unrepresentable at the write boundary, so the `debug_assert` is gone from core, the `if page_count > 0` short-circuit is gone from the UI, and the `tracing::warn!` that guarded the unreachable branch is gone with it. (The `open_file == None` warn in `app::OpenBookUseCase::run` is UNRELATED — it covers a different condition and remains.)

GENERAL PRINCIPLE: when an invariant is expressible as a type (`NonZeroUsize`, `Option`, a small enum), prefer the type — it makes the bad state unrepresentable at COMPILE time and removes the scattered runtime guards entirely. Fall back to the two-layer runtime pattern (core `debug_assert!` + UI precondition + warn) ONLY when the invariant is NOT type-expressible.

### Make the save path fallible end-to-end — never `unwrap_or` a serialize step (PR-La)

`Library::to_json -> Result<String, CoreError>` (symmetric with `from_json`), and `save`/`save_to` propagate it via `?`. A serialize step must NOT fall back (`serde_json::to_value(...).unwrap_or(Null)` / `to_string_pretty(...).unwrap_or("{}")`): that writes a TRUNCATED file to disk while the UI reports the save succeeded — silent data loss. Map each step to `CoreError::Library` and bubble it. (PR-T's `ThumbnailCache::get` swallowing a corrupt read to `None` is the deliberate OPPOSITE and correct there — a cache miss is recoverable; a primary-store save is not.)

### `CoreError` and `Library` are NOT `Clone` — use `match` to both keep a fallback AND surface the error (PR-La)

To recover from a failed startup load (fall back to a default) WHILE still surfacing the error message, you cannot write `result.clone().unwrap_or_default()` — neither `Library` nor `CoreError` is `Clone`, so it doesn't compile. Instead `match` the `Result`: the `Ok` arm moves the value out; the `Err` arm pushes the error's `Display` (`format!("{e}")`) into a `Vec<String>` of notices and substitutes the default. `main` does this for both `Settings::load` and `Library::load`, then surfaces the collected notices after the initial refresh (see the status-compose entry below).

### Move-only refactors — checklist of hard-won gotchas (PR-58 refactor set)

A "move-only" refactor (no behavior change, only file splitting) can still go wrong in four reproducible ways:

1. **Verify moved text against `git show <base>:<file>`, not the plan.** Plans that embed "exact content" often mis-transcribe Unicode (em-dash `—`, right-arrow `→` U+2192) as ASCII (`--`/`->`). Doc comments and string literals in the moved file must match the SOURCE byte-for-byte. (Note: the `\u{2014}` convention applies only to Rust *string literals*; doc-comment Unicode is kept as literal chars.)

2. **Let `clippy -D warnings` decide imports, not the plan.** Extracting functions can leave a type import UNUSED in production code when those functions were the only production callers — move that import inside the `#[cfg(test)] mod tests` block where the tests still name it. A plan step saying "keep all imports" can be wrong; clippy arbitrates.

3. **Grep docs AND `crates/` for prose descriptions, not just identifiers.** After extracting OR DELETING a symbol, search ALL of `crates/` and `docs/` for both the identifier AND its prose description (e.g. "enum↔index helpers", "the carousel builder") — not just the Rust symbol name — and update or remove every reference. `docs/architecture.md` (the as-built module map) must gain a section per new module. Deletion is the trickier case: removing `progress_fraction` as a free fn left a stale `docs/patterns.md` reference and stale code comments that only a grep sweep uncovered (issue #60).

4. **Safety net = unchanged test count.** Run `mise exec -- cargo nextest run --workspace --profile ci` before and after each task; the "N tests run" number must be IDENTICAL (a move neither adds nor drops tests). A delta means the extraction clipped or duplicated a test body.

### Parallel no-cargo writer + single-verifier pattern generalizes to new-API feature additions (PR-60)

The fan-out approach proven on move-only refactors (checklist above) also works for NEW-API feature additions across a real compile-dependency chain — demonstrated by issue #60 (`ReadingProgress` value object wired through `Book::progress`/`register_opened` → carousel → `main.rs`). The key precondition: every write-agent codes against a FROZEN public-API block pasted verbatim into its prompt (exact `ReadingProgress` signatures, exact `register_opened` signature). When each agent's scope is one disjoint file and the API contract is locked, per-file correctness is independent of compile order even across real dependencies. Fan out one no-cargo writer per disjoint file in a single wave, then run ONE sequential verifier (`fmt` / `clippy -D warnings` / `nextest`) that reconciles any drift. The stale-reference sweep (checklist point 3) applies equally: grep `crates/` and `docs/` for a deleted symbol AND its prose description, not just moved ones.

### Transient value objects over already-persisted primitives — do NOT serialize the derived object (PR-60)

When a value object (`ReadingProgress`) is derived from already-persisted primitives (`Book.last_page` + `page_count`), keep it TRANSIENT — do not give it `#[derive(Serialize, Deserialize)]` and do not add it to the persisted struct. The serde shape of `Book` stays `{path, title, last_page, page_count}` only; `LIBRARY_VERSION` is unchanged. Lock this with a serde-shape REGRESSION TEST (`reading_progress_is_not_persisted`) that serializes a `Book` to JSON and asserts (a) the object has exactly `{path, title, last_page, page_count}` and (b) none of `progress`/`reached`/`fraction` leaked as keys. This catches a future accidental `#[derive(Serialize)]` on the value object before it corrupts stored data. The value object lives only in the `Book` public API (`Book::progress() -> ReadingProgress`) and is reconstructed from the primitives on each call — zero storage cost, zero migration risk.

### Strict type at the write boundary, plain `Option` at the read-side value object (#65)

Put the STRICT type only where bad data ENTERS the domain: `set_page_count(_, NonZeroUsize)` and `register_opened(_, Option<NonZeroUsize>)` reject a `0` count at the write boundary, so positivity is guaranteed by the compiler at the one place it matters. But keep the DOWNSTREAM value object loose: `ReadingProgress::total` is `Option<usize>`, NOT `Option<NonZeroUsize>`. The value object is already downstream of the guarded boundary, so its total is known-positive-or-`None` in practice; tightening it would force every display/test consumer to call `.get()` or construct a `NonZeroUsize` just to read a number, for no reachable bug. To stay safe regardless, `fraction()` keeps a defensive `Some(0) => 0.0` arm (`Some(t) if t > 0 => …, _ => 0.0`) with a test pinning that arm. Rule: tighten the type where data is WRITTEN; leave the read-side value object holding the plain primitive so the strict newtype doesn't leak into code that just wants the number. (Both PR reviewers converged on this altitude.)

### Keep the storage primitive for serde byte-compat; surface the domain type through the accessor (#65)

The persisted shape and the in-memory domain type are allowed to DIFFER, and the accessor is where they meet. `Book.page_count` stays a bare `usize` on disk (`0` = unknown, `#[serde(default)]`, `LIBRARY_VERSION` unchanged — see the `#[serde(default)]` section above), while the domain surface is `Option`. `page_count_opt()` is the seam that maps stored `0 → None`; nothing else reads the raw field's sentinel. This generalizes the transient-value-object idea (section above): there the derived object is reconstructed from primitives on each call; here the SAME field is reshaped (`usize → Option<usize>`) on read. In both cases the persisted bytes are untouched and the accessor owns the translation — no migration, no `LIBRARY_VERSION` bump.

### std-widgets render light unless the build sets a dark style (#70)

Slint `std-widgets` (`ComboBox`/`SpinBox`/`CheckBox`/`Button`) render in the default light-ish style and float brightly in the dark UI; there is no per-widget dark token — the style is a build-time choice. `crates/gashuu/build.rs` sets it: `slint_build::compile_with_config("ui/ViewerWindow.slint", slint_build::CompilerConfiguration::new().with_style("fluent-dark".into()))`. Dark options: `fluent-dark` / `material-dark` / `cosmic-dark`. Keep the call inside the existing 32 MiB stack-size build thread (the wrapper guards against Windows `STATUS_STACK_OVERFLOW` during Slint lowering). Token-driven replacements for std `Button` are deferred to the P2 design PR.

### CI guard scripts: fail loud, never false-green (#70)

Two silent-failure traps when a bash guard scans files. (1) `grep ... || true` swallows grep's *error* exit 2 (e.g. an unreadable file) along with the benign no-match exit 1 — distinguish them: `matches="$(grep ...)" || { rc=$?; [ "$rc" -eq 1 ] || fail "..."; }`, so an unscanned file can't pass as clean. (2) Treat "0 files scanned" as a failure, not success, so a wrong path / empty glob can't false-green. Run under `set -euo pipefail`. Note: `var="$(cmd)"` (non-`local`) does NOT mask `cmd`'s exit code — only `local var="$(cmd)"` does, so don't cargo-cult a separate pre-init for the non-local form.
