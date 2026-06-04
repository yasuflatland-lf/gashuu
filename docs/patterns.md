# Patterns & gotchas (learned the hard way)

This is the L2/L3 reference doc migrated from the CLAUDE.md "Patterns & gotchas" section.
An agent should read the relevant entry BEFORE editing the corresponding code area.

### Cross-crate mocking via a `testing` feature

`gashuu-core` gates `mockall::automock` on `PageSource` behind `[features] testing = ["dep:mockall"]`; `gashuu`'s dev-dependency enables it, so `ViewerState` tests use `MockPageSource` without pulling `mockall` into release builds.

### `#[allow(dead_code)]` on test-only accessors

In a *binary* crate `pub` is not a public API surface, so `-D warnings` flags an accessor used only by `#[cfg(test)]` code as dead; such `#[allow(dead_code)]` is intentional and documented in place. (PR8a's thumbnail-strip wiring now USES `ViewerState::page_count()`/`index()` at runtime, so they shed their `#[allow]` — the pattern still applies to any future test-only accessor.)

### Bottom-up granular commits: a helper that lands BEFORE its caller needs a TEMPORARY `#[allow(dead_code)]`

When a feature is split into granular commits that land bottom-up (the `pub(crate)`/`pub` helper or method commits BEFORE the commit that wires its caller), the binary crate's `-D warnings` clippy gate flags the helper as `dead_code` in the intermediate commit — so an early commit fails the gates on its own. Add a TEMPORARY `#[allow(dead_code)]` carrying a NOTE that names the wiring task/commit, and have the WIRING commit REMOVE it as it adds the call site. This differs from the legitimately-permanent test-only-accessor `#[allow(dead_code)]` (entry below): this one is transient scaffolding, and the final tree must be FREE of it. Audit the final tree: `grep -rn "allow(dead_code)" crates/gashuu/src` should show only the documented permanent test-only accessors (e.g. `viewer_state.rs`'s test accessors, `navigation.rs`), never a leftover wiring-scaffold allow. (Contrast the `#76`/`#88` dead-helper entries below, which are about a helper that becomes PERMANENTLY dead — there the answer is delete-or-wire, not allow; here the helper is only TEMPORARILY dead between two commits of the same feature.)

### When one rule must hold across the Slint↔Rust boundary, make ONE side authoritative — don't mirror it (PR-S → #71)

PR-S originally MIRRORED the scrubber knob-fraction → page mapping: a pure Rust twin (`scrub_fraction_to_page`) was the unit-tested spec, and `Scrubber.slint` carried an EXACT-mirror `drag-page` expression (same clamp, same round-half-up, RTL inverting the fraction before rounding). #71 deleted the Slint side: the scrubber now passes the RAW clamped knob fraction (a `float` in `[0,1]`) up via `preview(float)`/`commit(float)`, and `on_scrub_preview`/`on_scrub_commit` in `main.rs` call `scrub_fraction_to_page` to resolve the page. So Rust is the SINGLE LIVE source of that mapping (clamp, RTL inversion, round-half-up all live there) — it has a real runtime caller and is no longer `#[allow(dead_code)]` / test-only. THE LESSON: a mirrored rule drifts (the two sides silently diverge on the next edit to one of them). When a single rule must hold on both sides of the Slint↔Rust boundary, make ONE side authoritative — let Slint pass the raw inputs across the boundary and compute the rule once in Rust, rather than re-deriving it in markup the cargo gates cannot test.

### Dead helper cleanup when a replacement takes over all call sites (#76)

When a PR swaps a helper `X` for a newer `Y` that serves all of `X`'s callers, an issue or plan that says "keep `X` intact for back-compat" is wrong against the gates: in a binary crate `-D warnings` rejects the now-unused `pub(crate) fn X` as `dead_code`, so the clippy gate fails. Remove `X` — don't keep it. The cargo gates are the arbiter of "dead," not the issue text; this is the "verify the as-merged state and reconcile the plan" discipline applied to a gate that *forces* the reconciliation. (#76's issue text said keep `thumb_image_at`, but it had only two callers and both were replaced by `thumb_state_at`, so the helper had to go.)

### Acceptance-criteria helper flagged dead by clippy → route production through it, don't delete (#88)

`clippy -D warnings` flagged `matching_indices` (required by the issue's acceptance criteria and covered by tests) as production-dead. The correct resolution: keep it and make production exercise it — `LibrarySearchState::recompute` delegates to `matching_indices` on the no-forced-paths fast path. Deleting a spec-required, tested helper just to satisfy the dead-code lint removes coverage and contradicts the acceptance criteria; wiring it into the single real call site satisfies both clippy and the spec. The gates are the arbiter of dead code (see the "Dead helper cleanup" entry above), but that means wiring, not deletion, when the helper is load-bearing by spec.

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
   without a UI (mirrors the `position_to_write_back` precedent). In PR-3 (#114) this was superseded
   by `OpenOutcome::Success(NoticesContent)` — `run()` now returns the neutral `NoticesContent` struct
   and `main.rs::finalize_open` formats it via `i18n::dynamic::format_notices`. The unit-test surface
   moved to the private `notices_content(...)` fn and its `#[cfg(test)]` block in `app.rs`.

3. **Preserve borrow discipline EXACTLY** (the moved body carries the same RefCell-drop choreography):
   single-statement `reconcile_settings(&state.borrow(), &viewport.borrow(), &mut s)`;
   `canonical = state.borrow().open_file()…` whose `Ref` drops at its `;`; `register_opened` →
   `jump_to` kept as separate statements on distinct `RefCell`s; refresh-BEFORE status-compose
   ordering; the `count_changed`-gated carousel rebuild.

4. **Slint gotcha:** the new submodule needed `use slint::ComponentHandle;` for `ui.as_weak()`. A
   submodule does NOT inherit the crate-root `include_modules!` trait scope that `main.rs` enjoys, so
   trait methods on generated Slint types must be brought into scope explicitly.

### Presentation state object: a named struct beats a tuple or an enum for Slint-setter fan-out (#76)

A 4th value-object flavor, distinct from the three above (`CacheConfig` invariant-owner, `SpreadContext` cohesion-wrapper, `OpenBookUseCase` use-case object): when a helper resolves one input into several values that the caller pushes into N *separate* Slint setters (`set_x_loaded`, `set_x_failed`, …), return a small `pub(crate)` struct with named constructors — not a bare tuple, not a full enum. `ThumbState` (#76, `carousel.rs`) returns `{ image, loaded, failed }` via `ThumbState::loaded(img)` / `loading()` / `failed()`, each constructor producing exactly one of the mutually-exclusive states.

Why not the alternatives:
- **Bare `(Image, bool, bool)` tuple** — unlabeled positions invite transposition bugs, and it can represent the impossible `(loaded && failed)` combo at every call site.
- **`enum Loaded(Image)|Loading|Failed`** — makes the invalid state unrepresentable, but each Slint setter wants a plain scalar, so the call site must `match` the enum straight back into `(image, bool, bool)` — more code, no clearer.

The struct splits the difference: the named constructors make the invalid combo unconstructible (no `match` at the call site — `ui.set_x_loaded(s.loaded)` stays declarative), while the fields remain the plain scalars the setters consume. Keep the fields `pub(crate)` while there is a single consumer; if a second consumer appears, switch to private fields + accessors to close the struct-literal bypass. The *source* model's own invariant (a `ThumbnailItem` is never `loaded && failed`) is enforced separately at its single constructor `thumbnail_item` (a `debug_assert`), so `thumb_state_at`'s loaded arm drops `item.failed` safely.

### State-holder that owns its recomputed projection: mutators recompute, callers read stale-never (#88)

A 5th value-object flavor — distinct from all four above. Use it when a struct carries a derived `Vec` (a filtered/projected slice of a larger aggregate) that MUST stay consistent with the struct's own query field. `LibrarySearchState` (#88, `carousel.rs`) is the canonical example: it keeps `visible_indices` (the filtered library rows in natural order). `set_query(query, &Library)` and `force_visible(paths, &Library)` mutate AND recompute `visible_indices` internally, so `visible_indices()` is never stale for the caller. The separate `pub(crate) recompute(&Library)` is ONLY for the library-changed-without-query-change case (startup seed, open-time page-count backfill). `force_visible` keeps freshly-added books visible until the next query change, and dedups to prevent double-entries.

The invariant owned here is FRESHNESS of the projection (contrast: `CacheConfig` clamps a primitive; `LibrarySearchState` keeps a derived `Vec` consistent). Lesson: a mutator that leaves a derived field stale for the caller to recompute is the same "recompute-and-re-find" smell the `#82` entry below warns about — just one call-site removed.

### Partial/total override pair: a persisted PARTIAL + a transient TOTAL with one merge rule (per-book view overrides)

A 6th value-object flavor — distinct from the five above (`CacheConfig` invariant-owner, `SpreadContext` cohesion-wrapper, `OpenBookUseCase` use-case object, `ThumbState` presentation-state, `LibrarySearchState` projection-freshness owner). Use it when a global config becomes a per-X override with a global fallback: model it as TWO types, not one.

- **The persisted PARTIAL** — `ViewOverride` (`view_override.rs`): one `Option<Enum>` per overridable field (`reading_direction`/`spread_mode`/`cover_mode`/`fit_mode`), where `None` means **inherit the global default** — an ACTIVE choice, NOT "unknown" (contrast `ReadingProgress::total`, where `None` is genuinely-unknown). `Copy`, immutable, `#[derive(Default)]` (= all-`None` = inherit-everything), with a named `none()` ctor for call-site intent. This is the type stored on `Book`.
- **The transient TOTAL** — `ResolvedView`: every field concrete (`Enum`, no `Option`), never persisted, produced ONLY by `ViewOverride::resolve(&Settings) -> ResolvedView`. The UI consumes it via `ViewerState::apply_resolved_view(ResolvedView)` (+ `ViewportState::set_fit(resolved.fit_mode)` for the viewport-owned field).
- **ONE merge rule.** `resolve` is the single definition of the per-field fallback (`self.field.unwrap_or(global.field)` per field). Because the only way to get a renderable view is `resolve`, "an unresolved view reaches the renderer" is UNREPRESENTABLE — there is no path that hands a partial `Option`-bearing value to the page pipeline.

The aggregate owns mutation: `Library::set_overrides(path, ViewOverride) -> bool` and `Library::overrides_for(path) -> ViewOverride` (there is NO `Book::set_overrides`). `set_overrides` returns the idempotent-changed bool (same `false` == no-op convention as `set_last_page`/`set_page_count`/`jump_to`); `overrides_for` returns an all-`None` `ViewOverride::none()` for an unknown path so the caller can `resolve` unconditionally.

Contrast with the read-side value object `ReadingProgress` (ADR-0006): that one is derived from already-persisted primitives and is transient; HERE the PARTIAL is the thing PERSISTED and the TOTAL is the transient derived form — the persisted/derived split runs the opposite direction. See conventions.md ("Backward-compatible additive serde field") for the serde shape and the three round-trip tests, and ADR-0007 for the design decision.

**Per-field merge ISOLATION test (guards a field-swap copy/paste bug).** A merge rule written field-by-field (`spread_mode.unwrap_or(global.spread_mode)`) is one transposed identifier away from a silent bug (`spread_mode.unwrap_or(global.cover_mode)`) — and it still TYPE-CHECKS when both fields share an enum-shape neighbourhood, or compiles fine and only mis-resolves at runtime. Pin it the way `view_override.rs` does: build a `global` with EVERY field set to a NON-default value, then assert each field resolves to its OWN global field — once for an empty override (all inherit) and once per field set in isolation (that field wins, the OTHER three still inherit the non-default global). A test where global == defaults would let a field-swap pass (the swapped value coincidentally equals the right default).

### Write-direction invariant audit when a setting goes per-context — every "copy runtime → global" op becomes a potential clobber (per-book view overrides)

THE KEY GOTCHA of this feature, and the reusable harness for any "global config X becomes per-context with a global fallback" change. When a setting that was ONLY ever global gains a per-context (per-book) form, EVERY pre-existing operation that copies runtime state INTO the global config (`reconcile_settings`) silently becomes a CLOBBER: the runtime now holds a per-context value, so reconciling it writes one book's preference over the GLOBAL default.

A real bug shipped-then-caught here. The plan correctly gated the EXIT reconcile on "no book open", but MISSED a SECOND reconcile on the open path — inside `OpenBookUseCase::run`, behind the `if track_recent_files` save gate. Runtime view modes are NOT reset when a new book is opened (the new book's `ResolvedView` is applied LATER in `run`), so at that save point the runtime still held the OUTGOING book's per-book modes — reconciling them there wrote the outgoing book's preferences into the global `Settings`. The fix was to NOT reconcile on the open-time save at all (it now persists only recents + cache/preload/track + the UNCHANGED global view fields); the comment at that call site spells out why.

HARNESS:

1. **Enumerate ALL sites that write the global config AND all that write the per-context override** — do NOT trust the plan's enumeration (the plan missed one here). Grep for both write fns (`reconcile_settings` and `write_back_view_override`) and read every call site.
2. **State the invariant explicitly, then audit every site against it.** The invariant for this feature: the GLOBAL view modes are written ONLY by (a) the global-scope UI — the Library settings dialog close — and (b) the no-book-open exit path; the PER-CONTEXT override is written ONLY at leave points (`write_back_view_override`). Any reconcile that does not match (a)/(b) is the bug.
3. **Watch for the asymmetry that hides the clobber**: runtime modes persist across an open (they are re-seeded only AFTER the open succeeds), so "the runtime obviously holds the global default at a global-write site" is FALSE on the open path. The dangerous sites are the ones where a global-write runs while the runtime still holds a per-context value.

This is a sharper instance of the "verify the as-merged state, do not trust the plan" discipline (the move-only-refactor checklist + the `#76` dead-helper entry): here the plan's enumeration of write sites was the thing that was wrong.

### Single visible-index projection for filtered views — never re-sort the filtered slice (#88)

When search/filtering is active, project filtered indices through `Library::books()` order; do NOT re-sort the filtered subset independently. `visible_indices()` is the single source of truth; carousel open/move, `cover_requests`, and `build_carousel_model` all resolve through it.

For unit-testability, `build_carousel_model` was split into a HEADLESS builder (`fn build_carousel_model_from(indices: &[usize], …) -> VecModel<…>` — no Slint handle required) and `bind_carousel_model` (performs the Slint bind). `cover_requests(library, indices)` re-bases each `row` to the enumerated position in the filtered slice, NOT the raw library index — an off-by-one here silently loads the wrong cover. This is the same "aggregate owns its ordering, UI inherits it" discipline as #82, applied to a filtered projection.

### Return the stored authoritative value from a mutating method; don't recompute-and-re-find in the caller (#82)

When a method canonicalizes/normalizes its input and stores a derived value (e.g. `Library::add` canonicalizes a path before storing the `Book`), have it RETURN that stored value (`add` returns `Option<&Path>` — the stored canonical path, `None` on duplicate) rather than a `bool`. A caller that re-derives the key itself (a second `canonicalize`) and then `find`s the item the callee just inserted can silently diverge from the callee's own derivation — a filesystem TOCTOU, symlink, or case difference between the two `canonicalize` calls makes the lookup miss, so the item is stored but dropped from the caller's result set with no error (here: the added book vanishes from the focus/count path). One source of truth: the mutator returns what it stored; the caller consumes it (`add_paths` is now `filter_map(|p| lib.add(p).map(Path::to_path_buf))`). This also removes the redundant second syscall and the O(n) re-find.

Smell to watch for: a caller computes a key, calls a mutator, then searches for what the mutator just created.

### A core aggregate owns its ordering invariant; the presentation layer inherits it via the single accessor (#82)

When display order must stay consistent, make the core aggregate the one place that sorts: `Library` sorts in `add()` and exposes only `books()` in sorted (natural title, canonical-path tiebreak) order. The UI builders (`carousel_data`, `cover_requests`) iterate `books()` and inherit the order with no sort of their own, so carousel rows and cover-request indices stay aligned automatically. Do not sort in the presentation layer — a second sort site is a divergence risk: when the aggregate's sort changes, the presentation-side sort may produce a different order, and the two silently disagree on index-to-item mapping.

### Normalize-on-load when adding an ordering invariant to a `#[derive(Deserialize)]` type (#82)

`serde` builds the struct field-by-field, bypassing `new()`/`add()`, so an ordering invariant established only in those constructors is NOT applied to deserialized data. Add a `pub(crate) fn normalize(&mut self)` that re-establishes the invariant (here: re-sort `books`) and call it on the load path (`library_store::load_from`) right after `from_value`/`from_str`. This is the same discipline as the `CacheConfig` no-Deserialize rule, applied to a type that must carry `#[derive(Deserialize)]`: the type cannot shed the derive, so the invariant is re-enforced after deserialization instead. Bonus: calling `normalize` on load upgrades data persisted before the invariant existed — insertion-ordered libraries converge to natural order on the next save, with no migration version bump.

### Changing an ordering invariant silently flips order-dependent test fixtures (#82)

Tests that index by position (`lib.books()[0]` vs `[1]`) or assert per-row flags (`needs_count`, `available`) keep compiling after you change the sort order, but now assert against the OLD order. When you change ordering, audit every order-dependent fixture: rename intent-revealing tests (`add_appends_in_insertion_order` → `add_orders_books_by_natural_title`) and re-derive expected indices from the new order (e.g. `known.cbz` now sorts before `unknown.cbz`, so the per-row flags swap). A test that still passes after a sort change should be treated with suspicion — check that its assertions actually exercise order-sensitive behaviour, not an accidentally order-independent property.

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

`NoticesContent.skipped` (carried by `OpenOutcome::Success`) + `finalize_open` in `main.rs` appending the formatted notice after `refresh()`, via `get_status_text`/`set_status_text`. WHY: `tracing::warn!` alone is invisible in a GUI run (`RUST_LOG` is usually unset). (Pre-PR-3 this used `ViewerState::last_open_skipped()` — that getter is gone; the skip count now flows through `NoticesContent` as a pure data field.)

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

### Pointer-driven auto-hide chrome FLIP-FLOPS under a stationary cursor — guard the reveal AND pause idle on hover (viewer glass-pill)

`PageView` reveals chrome on `changed mouse-x/mouse-y`. When the idle-`Timer` hides the chrome out from under a RESTING cursor, the just-vacated pill/title lets `PageView`'s `TouchArea` re-acquire the cursor and emit a PHANTOM `changed mouse-x` (a fake "move" with no real pointer motion) → the chrome re-reveals → it flip-flops every idle interval. The fix is TWO complementary guards, both required:

1. **Suppress pointer-driven reveals for a short window after an idle-hide.** A `suppress-pointer-reveal` bool is set on idle-hide and cleared by a one-shot `reveal-guard` `Timer` (~250 ms); `PageView`'s pointer-move routes through a guarded `reveal-chrome-from-pointer()` that no-ops while suppressed. Reveals that carry real user INTENT — arrow keys, scrubber drag — bypass the guard and reveal directly. This swallows ONLY the phantom move that fires the instant the chrome vacates the cursor.
2. **Pause the idle-`Timer` entirely while the pointer hovers the chrome.** A `chrome-hovered` bool keeps the `Timer` from running while the user is pointing AT the chrome, so chrome the user is actively targeting never hides under them. Aggregate the hover signal from EVERY interactive child (each `NavItem.hovered`, a field `TouchArea`, AND a background `TouchArea` covering the gaps between items) — a hover bool sourced from only the buttons leaves the inter-item gutters un-hovered and the chrome hides mid-reach.

Guard (1) alone still hides chrome the user is reaching for; guard (2) alone still flip-flops once the cursor finally rests OUTSIDE the chrome. Both live in `ViewerWindow.slint` (`suppress-pointer-reveal` + `reveal-guard` + `chrome-hovered`).

### Scrubber drag is preview-on-move, commit-on-release (PR-S)

During a scrubber drag, ONLY the preview popover updates: `preview(float)` resolves the raw fraction to a page (via `scrub_fraction_to_page`) and pulls thumbnails from the existing `VecModel<ThumbnailItem>` — it must NEVER call `jump_to`/`refresh`. The page body changes ONLY on knob release via `commit(float)` → `jump_to` → `refresh`. Keep all decode/navigation side effects on the commit path; preview is display-only and UI-thread-only (the `Rc`/`!Send` thumbnail model is never crossed). (Both callbacks carry the RAW clamped fraction, not a page index — see the authoritative-side boundary entry above.)

### Progress chrome (a filled track) DERIVES from the single-source knob geometry — no parallel state, reading-direction aware, with a degenerate-fill guard (scrubber HIG restyle)

A visual that must track an interactive value should be DERIVED from the existing single-source geometry, not given its own state. The scrubber's HIG filled track is one `Rectangle` whose span is computed from `knob-cx` (itself `knob-frac`-based) — so it tracks the pointer LIVE during a drag and the idle anchor otherwise, with ZERO new state, and the page body still does not change during drag (the fill is chrome on the scrubber, not a page jump). Select fill DIRECTION from the existing `rtl` prop: RTL (manga) fills knob→RIGHT rail edge (page 1 sits screen-right), LTR fills LEFT edge→knob. GOTCHA/HARNESS: guard the fill to ZERO width when `total-pages <= 1`. The LTR degenerate is naturally zero (`knob-cx - rail-x` at `idle-frac == 0`), but the RTL degenerate is a trap — `idle-frac` guards to `0.0` → knob pins screen-left → the unguarded `(rail-x + rail-w) - knob-cx` spans the FULL rail, i.e. "100% read" on an empty/1-page book. Z-order: rail → fill → knob (knob on top); `radius-xs` on the fill matches the rail so full-fill aligns with no 1px clip.

### Only the INSTANTIATED root window's surface is reachable from Rust — re-expose child properties/callbacks on the root (PR-L)

Slint's generated Rust API exposes ONLY the properties/callbacks/`public function`s declared on the window component `main.rs` instantiates (`ViewerWindow`). A child component's internal `in property`/callback (e.g. `Carousel.items`, `Carousel.add-books()`) is INVISIBLE to Rust — there is no generated accessor for it. To wire a child property/handler from Rust, declare a twin on the ROOT and bind/forward it to the child: `ViewerWindow` exposes `in property <[CarouselItem]> carousel-items` bound by `items: root.carousel-items;`, and root `add-books()`/`add-folder()` callbacks forwarded into the `Carousel`. Generated name mapping: kebab→snake_case, `set_<prop>`/`get_<prop>`, `on_<callback>`, `invoke_<public function>` (e.g. `set_carousel_items`, `on_add_books`, `invoke_focus_carousel`). When adding a new Rust-driven property/handler, put it on the root window first — not only on the child.

### A callback SIGNATURE change ripples to THREE places, not one (#71)

A child-component callback whose type changes (e.g. the scrubber's `preview`/`commit` going from `int` to `float` when #71 moved fraction→page rounding into Rust) must be edited in all THREE: (1) the child component `.slint` declares the callback; (2) the `ViewerWindow` root TWIN callback `.slint` re-declares + forwards it (Rust binds only the root window surface — see the entry above, so the child's callback alone is invisible to Rust); and (3) the Rust closure(s) (`on_scrub_preview`/`on_scrub_commit`) that receive the new type. Miss any one and it either won't compile (Rust closure type mismatch) or won't wire (an unforwarded root twin). Search for both the child name and the kebab→snake twin name when changing a callback's type.

### `if`-gated element ids are NOT reachable from the parent's `public function`s / `init` — gate with `visible:` when an id must be parent-reachable (PR-0b)

Slint scopes an id declared inside an `if`/`for` branch to a child the enclosing component cannot name, so a parent-level Rust-invoked seam like `focus-pages()`/`focus-carousel()` (or `init`) CANNOT `.focus()` an element under `if cond : Foo { ... }`. When a screen/region must be referenced by id from a parent function or `init`, gate it with `visible: <cond>` (keeps the id at root scope) instead of `if <cond>`. Trade-off: `visible:` keeps every branch instantiated (both screens live in the tree, toggled by visibility) — accepted here; focus is driven explicitly by the Rust seam functions on each transition. PR-0b's `ViewerWindow.slint` gates the Carousel (screen 0) and the Viewer body (screen 1) with `visible: root.screen == N` precisely so `focus-carousel()`/`focus-pages()` can reach `carousel`/`fs`.

### `root` is the COMPONENT root; `parent` is the IMMEDIATE enclosing element only — count the nesting when reading a `for`-item property (PR-C, slint 1.16.1)

`root.<name>` resolves to the component's root element, NOT to a `property` declared on a nested element such as a `Repeater` `for`-item `Rectangle`. To read a property declared on the for-item element from a DIRECT child, use `parent.<name>`. (Bug: a `private property <length> row-cy` on the cover-flow row `Rectangle` was wrongly read as `root.row-cy`; the fix is `parent.row-cy`.) And `parent` climbs exactly ONE level: a property on the for-item `Rectangle` (e.g. a per-item `focused: bool`) is NOT reachable from a GRANDCHILD via `parent.<name>` — at that depth `parent` is the intervening element. (Bug: a per-`Image` `colorize: parent.focused ? …` failed because `parent` there was the inner cover `Rectangle`, which has no `focused` — that lives two levels up on the for-row `Rectangle`; the recede cue was carried by the for-row's `opacity` instead.) Neither error is caught by the cargo gates (see the spec-by-hand entry below); both compile-fail only at `build.rs` Slint compile or render wrong silently. THIRD nuance (cover-flow z-order rework): a component's OWN root element cannot read `parent` AT ALL — the parent is unknown until instantiation, so `x: parent.width …` on a `component Foo inherits Rectangle { … }` root fails with `Cannot access id 'parent'` (and a cascade `Cannot convert float to length`). Pass the enclosing geometry IN as `in property`s set at the `for`/instantiation site (where `parent` IS the enclosing element): `CoverCard` takes `row-width`/`row-cy` set from `parent.width`/`parent.row-cy` and uses `root.row-width` internally. (`parent` from a NESTED element inside the component still works — only the component root is blind to it.)

### Slint 1.16.1 accepted syntax + limitations (verified at impl time, PR-C)

Confirmed ACCEPTED by the pinned Slint 1.16.1 (future work need not re-verify): `Math.clamp(x, lo, hi)`, `overflow: elide` on `Text`, `color.with-alpha(f)`, 8-digit `#rrggbbaa` hex, and `@linear-gradient(deg, stop% … )`. NOT supported: per-`Repeater`-item z-order is not settable in Slint 1.x — layer a focused item via opacity/size/accent-ring, not z. To force a focused `for`-item to paint ON TOP of its overlapping neighbors (which model-index draw order otherwise leaves partly covered), render the model with TWO `for` passes over the SAME model into one shared file-private sub-component: pass 1 paints the neighbors (`show: idx != focused-index`), pass 2 — declared AFTER pass 1, so painted later/above — paints ONLY the focused item (`show: idx == focused-index`); `show` gates `visible`, nothing else. Because BOTH passes bind identical geometry, each item keeps a persistent instance in each pass, so a focus-change SLIDE still animates (a `visible:false` Repeater instance keeps computing/animating its geometry, so the hand-off between passes lands at matching positions — no pop). Cost: 2N instances (invisible ones aren't drawn). Do NOT instead reorder the model or use a standalone always-centered focused element — both destroy the slide (a new/destroyed instance snaps with no transition). `Carousel.slint`'s `CoverCard` two-pass does exactly this.

**CORRECTED two-pass z-order pattern — always-on pass-1 backing layer (fast-scroll gap fix):**

The original `show: idx != focused-index` formulation had a subtle render-timing bug:
toggling pass-1 visibility ON EVERY FOCUS CHANGE blanked each passed-through book's cover
for exactly 1 frame (the frame where pass-1 hides it and pass-2 has not yet painted it from
the new focused position). Fast scroll chains these 1-frame blanks into a visible
"gap-toothed strip" effect.

The CORRECTED pattern:
- **Pass 1 is an always-on BACKING layer**: `show: true` (every book, every frame).
  Set `elevated: false` (or equivalent flag) so semi-transparent overlay effects
  (shadow, tint, highlight ring) only apply to the pass-2 copy.
- **Pass 2 paints only the focused card on top**: unchanged — `show: idx == focused-index`,
  declared AFTER pass 1 so it paints above it; carries `elevated: true` and all
  focus-specific effects.

WHY the always-on approach fixes the gap:
- No book ever loses its only visible instance during a sweep — pass-1 always covers every
  slot, so a fast scroll that moves focus across N books never blanks any of them.
- Transient render gaps (1-frame blank) = render-TIMING cause → fix by keeping the
  backing layer always visible.
- Permanent gaps (data/model missing) = DATA cause → fix differently (model integrity).
  Use this framing when diagnosing future z-order render bugs.

HOW to apply in future Slint two-pass z-order components:
1. Keep pass-1 `show: true` always — never gate it on `idx != focused-index`.
2. Use an `elevated` (or equivalent) flag to gate semi-transparent/overlay effects
   to the pass-2 (top) copy ONLY — avoid double-compositing semi-transparent layers.
   Opaque layers are safe to double-draw at identical geometry (no visual artifact).
3. If a focused-only GEOMETRY change is needed (scale, offset), apply it on pass-2;
   pass-1 always renders at the base geometry.

no `line-height` on `Text` in Slint 1.x — DESIGN's per-role `lineHeight` cannot be expressed and must not be faked (space elements apart instead). A shared PRIVATE (non-exported) sub-component (`component Foo inherits Rectangle { in property … }`) cleanly de-duplicates repeated markup WITHIN one file WITHOUT touching any exported struct/component contract, and works both absolutely positioned and as a layout child (PR-C used a file-private `ProgressBar` for the per-cover and focused-meta bars in `Carousel.slint`). When the same markup must be reused ACROSS files, promote it to an `export`ed component under `ui/components/` instead (#71 did exactly this — `ProgressBar` is now `components/ProgressBar.slint`, imported by `Carousel.slint`); keep the file-private form only when the reuse is confined to one file.

### `states[]` cannot assign a child element's property or read `parent`/sibling geometry (#83, slint 1.16.1)

In `NavItem.slint`, setting a CHILD element's property from the root's `states[]` block (`icon-img.colorize: …`, and a pressed `icon-img.y` nudge) failed to compile: `Cannot access id 'parent'` plus a cascade `Cannot convert float to length`. A `states[]` block resolves ids in the ROOT element's scope ONLY — a named child/sibling and `parent` are NOT reachable there (the same scoping limit as the `root` vs `parent` entry above, here applied to `states[]`). HARNESS/FIX: keep `states[]` to ROOT-level properties (`background`, `border-color`), and drive a child element's state-dependent property with a DIRECT binding ON the child that reads the touch/condition instead — `colorize: touch.has-hover || touch.pressed ? Theme.text-high : Theme.text-mid;`. This is behavior-identical and compiles. The optional pressed "sink" that moved the icon's `y` was dropped: a state-driven child GEOMETRY change has the same limitation, so there is no `states[]` form of it. SAME FIX, applied to an element's OWN geometry (scrubber HIG restyle): the knob's drag-grow (16→20px while pressed) reads the SIBLING `ta.pressed` DIRECTLY in its `width`/`height` bindings (`ta.pressed ? Theme.scrubber-knob-size-active : Theme.scrubber-knob-size`) with an `animate width, height`, NOT a `states[]` block; because `x`/`y` are bound to `self.width/height` (`root.knob-cx - self.width / 2`), the knob stays centered as it grows. Prefer a direct sibling/condition binding over `states[]` whenever the target is a child OR a sibling-driven self property.

### First image assets: `@image-url` + `colorize` for single-path SVG icons (#83)

#83 introduced the repo's FIRST image assets (`crates/gashuu/ui/assets/file.svg`, `folder.svg`) and FIRST `@image-url` usage (the repo had none before). HARNESS: `@image-url("assets/file.svg")` resolves RELATIVE to the `.slint` file that contains it — it lives in `Carousel.slint` (in `ui/`), so it points at `ui/assets/`. Keep the `@image-url` in the CONSUMER that owns the asset-relative path, and pass the resolved value DOWN to leaf components as `in property <image>` (NavBar takes `file-icon`/`folder-icon`, NavItem takes `icon`) so leaf components stay asset-path-agnostic and reusable. Single-color, single-`<path>` SVGs are recolored at use via `Image { colorize: <Theme color>; }` — the SVG's own `fill` is IGNORED, so the icon picks up `Theme.text-mid`/`text-high` like any token-driven color (no inline hex; check-tokens stays green because the hex lives only inside the `.svg` file, which the guard does not scan as `.slint`). Author the SVG at a generously large logical size (icon shown at 21px, viewBox 24) — being vector it stays crisp on HiDPI. `build.rs` needs NO change: it compiles the single entry `ui/ViewerWindow.slint` and `@image-url`/imports cascade (see the "compiles only what is REACHABLE" entry below); SVG assets are embedded at compile time (text, no committed binaries).

### Slint `opacity` rasterizes to an offscreen layer that ignores the scale factor → blurs child SVG/text on HiDPI (viewer glass-pill)

An element with an `opacity` property (even `opacity: 1.0` paired with `animate opacity`) is composited via an OFFSCREEN layer. In Slint 1.16's femtovg/GL backend that layer is NOT rendered at device-pixel resolution, so SVG icons and text inside it look blurry on Retina/HiDPI. Concrete: the Library `NavBar` (no opacity) stayed sharp while the Viewer's `ViewerPill` (opacity fade) was blurry rendering the SAME `settings.svg`. HARNESS: do NOT wrap icon/text-bearing chrome in `opacity` for fades. Gate auto-hide with `visible:` ONLY (instant show/hide, no offscreen layer), or fade ONLY a plain background layer that contains no icons/text. This is why the `ViewerPill`/title auto-hide uses `visible:` without opacity.

### Slint `Image` rasterizes an SVG at its intrinsic size, THEN scales — a small intrinsic size blurs on HiDPI (viewer glass-pill)

A Slint `Image` rasterizes an SVG at its INTRINSIC size (`width`/`height` attrs, falling back to `viewBox`) and only then scales the bitmap to the layout box — so a small intrinsic size upscales blurry on HiDPI (refs: slint-ui/slint #734, discussion #7769). As cheap insurance the icon SVGs were enlarged to 96 intrinsic px (`viewBox` kept at 24). NOTE: in the glass-pill case the DOMINANT cause was the opacity offscreen layer (entry above), NOT intrinsic size — the 96 px is documented here only so future icon work STARTS at a safe intrinsic size. The Slint maintainer's real fix for SVG blur is "don't use the software renderer"; gashuu is on femtovg, so this is belt-and-suspenders, not the primary lever.

### A single `padding` on a Slint `HorizontalLayout` insets ALL FOUR sides — use `padding-left`/`padding-right` for a fixed-height pill (viewer glass-pill)

A lone `padding: X` on a `HorizontalLayout`/`VerticalLayout` eats TOP and BOTTOM too, not just the horizontal sides. Inside a fixed-height capsule (34 px), a `padding: 13px` left only ~8 px of vertical room and CLIPPED 12 px-tall digits. HARNESS: for a horizontal pill of items, pad with `padding-left`/`padding-right` ONLY so the full capsule height stays available for vertical-centering; never use the four-sided shorthand inside a height-constrained capsule. SECOND INSTANCE (settings segmented, fix 2026-06-04): `padding: space-xxs` inside the 30 px Segmented capsule squeezed the cells to 22 px and clipped the labels' ascenders; fixed with horizontal-only padding plus the selected pill demoted to a vertically inset CHILD of the now full-height cell (so the accent platter keeps its HIG inset while the label centers in the full 30 px).

### Slint 1.x `Text` has no text-shadow — glow a label with offset duplicates, NOT a `Rectangle` drop-shadow (viewer glass-pill)

Slint 1.x `Text` has no `text-shadow`, and a `Rectangle`'s `drop-shadow-*` paints a soft-edged full-width BAR behind the text box, not a per-glyph glow. To glow a book-title label so it stays legible over bright artwork, draw the text SEVERAL times in the glow color (`Theme.title-glow` `#3d3d3d`), offset ~1.5–2 px in 8 directions (a `for` loop over offset structs), BEHIND a single crisp white copy. A backing capsule behind the title is the alternative but was rejected by the design here (the floating title should read directly over the art). `Theme.title-glow` carries an explanatory comment so nobody mistakes it for an accent token.

### A Slint `Rectangle` paints exactly ONE drop-shadow and has NO spread radius (scrubber HIG restyle)

A single `Rectangle` renders ONE `drop-shadow-*` and there is no shadow SPREAD parameter — so you cannot stack a crisp accent ring AND a separate dark depth-shadow on one rectangle, and you cannot express a `0 0 0 Npx`-style spread ring. Two realizations: (1) a SOFT halo = one `drop-shadow-color`/`drop-shadow-blur` reusing `Theme.accent-glow` (the system's single glow) applied directly on the element — this is how the white scrubber knob carries its accent halo; (2) a CRISP ring requires a nested, slightly larger tinted `Rectangle` placed BEHIND the element, not a shadow trick. Decide soft-vs-crisp at visual review and default to the soft halo. (Distinct from the `Text`-glow entry above, which is about `Text` having no per-glyph shadow at all.)

### A transparent field background blends an input INTO the surrounding glass pill (viewer glass-pill)

A `surface-sunken` fill made the `NavBar` search field and the `ViewerPill` page-jump field read as a darker, boxed-in input sitting ON the pill rather than part of it. `background: transparent` (and, for the page-jump field, dropping the border too) makes the field read as an integral region of the glass pill. The border/box around a text input is OPTIONAL chrome, not a requirement — omit it when the input lives inside an already-bounded surface (a pill/capsule) so the two don't double up.

### Slint search-field harness: `TextInput` + 120 ms trailing debounce + focus discipline (#88)

Use `TextInput` (not `LineEdit`) so all colors are `Theme.*` tokens; `LineEdit` hard-codes light-widget colors that fight the dark theme. Debounce with a `Timer` re-armed (`timer.restart()`) on every `edited` event and stopped (`timer.running = false`) on `accepted` / Esc / Down; the Rust `set_query` fires only when the timer triggers (120 ms trailing). Enter and Down move focus back to the carousel WITHOUT opening a book (Enter triggers open in the carousel `FocusScope`, not the search field). A no-results panel is distinct from the empty-library CTA: gate it on `library-book-count > 0 && items.length == 0` so first-launch and post-search-empty never show the same copy. The in-field search icon is an `@image-url` `Image` colorized with a `Theme` token — same `@image-url` + `colorize` pattern as #83's nav icons; do NOT bake the color into the SVG.

### Accessibility for a mouse + screen-reader control kept OUT of the keyboard focus chain (#83)

#83's nav is "mouse + screen-reader oriented"; keyboard navigation stays owned by the carousel `FocusScope`. This is the repo's FIRST use of Slint accessibility hooks. HARNESS: a NON-focusable `TouchArea` + `accessible-role: button` + `accessible-label` exposes the control to assistive tech WITHOUT adding it to the Tab/keyboard focus chain (in Slint 1.x only `FocusScope`/focusable widgets enter that chain, so the carousel keeps keyboard ownership). But `accessible-role`/`accessible-label` ALONE only let AT READ the label — to make AT ACTIVATION fire the action, wire `accessible-action-default => { root.clicked(); }` so a screen-reader activation gesture invokes the SAME callback as a pointer click. The `accessible-label` is NOT rendered on screen.

### The cargo gates do NOT exercise Slint markup behavior — verify `.slint` logic against the spec by hand (PR-0b)

fmt/clippy/nextest cover Rust only; Slint key handlers, bindings, and visibility live in `.slint` markup that compiles via `build.rs` but has NO automated behavioral test (the project does not unit-test Slint visuals). After editing a `.slint` `FocusScope` key handler or property binding, explicitly check it against the spec — a missing key arm compiles and passes ALL three gates silently. Concrete PR-0b miss: the `Key.UpArrow -> nav("up")` arm (the entire point of the GoToLibrary feature) was initially omitted from the viewer `FocusScope` yet every gate stayed green; it was caught only by spec re-reading.

### Slint compiles only what is REACHABLE from the entry file — create-and-consume are verified together (#71)

`build.rs` compiles the single entry `ui/ViewerWindow.slint`; `import` statements cascade to pull in only the files reachable from it (which is why adding the new `ui/components/` atoms/molecules needed NO `build.rs` change). The flip side: a component under `ui/components/` is NOT compiled until some reachable file imports it, so a standalone component's syntax errors surface ONLY on its first consumption. Treat create-and-consume as one step — adding a component AND wiring its first consumer in the same change is what actually exercises the new file; an unimported component can sit broken with every gate green.

**Staged-PR reachability guard — TEMP export + load-bearing compiler warning (issue 127):** When a PR split requires a component to be defined in one PR and consumed in a later one, add a TEMP `export { Foo }` in `ViewerWindow.slint` to keep the component reachable from the build entry point — otherwise the file is never compiled and syntax errors are invisible. The Slint compiler emits `Exported component 'Foo' doesn't inherit Window. No code will be generated for it` on `build.rs` stderr; because that warning travels through the build script, it does NOT reach cargo's `-D warnings` gate, so the build stays green. This is INTENTIONAL: the warning is a load-bearing signal that the guard is active. When the consuming PR wires the component into the live markup, the TEMP export becomes unnecessary (the component is now reachable via the import chain); add a `grep -rn "export { Foo }" ui/` → 0-hits check to the consuming PR's acceptance criteria to enforce removal.

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

The page `FocusScope` key handler guards `if (show-settings || show-guide || show-shortcuts) { return reject; }` so background nav keys don't drive the hidden viewer while a modal is up — EVERY open flag must be in that guard (and in the carousel `modal-open`); closing an overlay calls `ui.invoke_focus_pages()` (the overlay counterpart of the Button `fs.focus()` rule; `focus-pages()` exists since PR8a) — return focus to the RIGHT scope for the CURRENT screen (carousel on screen 0, pages on screen 1; see the close-returns-focus entry below). The "Settings…" button deliberately omits `fs.focus()` (the dialog needs focus). The settings dialog now implements ALL THREE dismiss paths (issue 103, PR-B reversed the earlier "button-only" deferral): Esc (the dialog's own `fs` `FocusScope`), backdrop-click (a scrim `TouchArea` outside the panel), and the Close button — all three call the single `close-settings()` callback. More than one modal can be up at once (the shortcuts overlay stacks ON the still-mounted settings dialog — see the modal-over-modal entry below). See the settings-glass-panel harness entries below for the FocusScope-ancestor, focus-on-open, and backdrop-without-swallowing-clicks mechanics.

### A new modal entry point over a screen with its own `FocusScope` must guard that screen's key handler — and close must return focus to the RIGHT scope (#88)

When a modal (e.g. SettingsDialog) becomes openable from a NEW entry point over a screen that has its own `FocusScope` (the Library carousel), that screen's `key-pressed` handler must mirror the same guard the Viewer already carries — `if (root.modal-open) { return reject; }`, bound to ALL open conditions (`show-settings || show-guide`) — otherwise keys leak behind the modal (e.g. Return opened a book under an open dialog). Also: a modal's CLOSE handler must return focus to the RIGHT scope based on the CURRENT screen (carousel on Library, pages on Viewer), not unconditionally to the prior screen's scope — otherwise keyboard nav silently dies after dismissing the dialog from the Library. Both failures compile and pass all three gates (see the "cargo gates do not exercise Slint markup" entry). This is the same class as the existing Button-click focus rule: focus is an invariant the code must maintain explicitly at every transition, not one the runtime restores automatically.

### Custom keyboard-operable control atoms: one `FocusScope` + explicit `fs.focus()` on click + a soft `accent-glow` focus ring + root-level a11y (issue 102, PR-A)

`Segmented` / `Stepper` / `Toggle` (drop-ins for the settings dialog's native `ComboBox` / `SpinBox` / `CheckBox`) are the repo's FIRST custom controls that are BOTH clickable AND keyboard-operable — `NavItem`/`PrimaryButton` are click-only TouchAreas. The reusable harness:

- **One `FocusScope` per atom = one Tab stop.** It expands to the root by default and owns the `key-pressed` handler (←/→ for Segmented, ↑↓ / `+`-`-` for Stepper, Space/Enter for Toggle). The inner cells / ± buttons use NON-focusable `TouchArea`s so the whole atom is a SINGLE focus stop, not N.
- **A `TouchArea` consumes the press, so `FocusScope`'s `focus-on-click` does NOT fire — call `fs.focus()` explicitly in every `clicked` handler.** Same class as the Button-click focus rule above; after a click the atom holds focus, so click-then-arrow works. The atom is Tab-reachable via `focus-on-tab-navigation` (default true).
- **Focus ring = `accent` border + ONE soft `accent-glow` drop-shadow** (the single-drop-shadow / no-spread constraint above). `border-color` / `drop-shadow-color` / `drop-shadow-blur` are DIRECT bindings reading the sibling `fs.has-focus` (a `states[]` block can't read a sibling — see the states[] entry); only `border-color` is animated (animating drop-shadow is version-fragile, per NavItem). NO `opacity` anywhere — selection animates `color`, the toggle knob animates `x` (the HiDPI offscreen-blur entry).
- **a11y mirrors the NavItem idiom, scaled up:** role on the ROOT (`combobox` / `spinbox` / `switch`), `accessible-label`, and an action that fires the SAME mutator a click does — `accessible-action-default` (Toggle), `-increment`/`-decrement` (Stepper), or per-cell `list-item` + `-action-default` (Segmented). `Toggle` also binds `accessible-checked`. Slint 1.16.1 accepts these roles plus `accessible-checked`/`-value`/`-action-*` — VERIFY a new accessible prop against the `i-slint-compiler` builtins first, because a wrong one fails only at the `build.rs` Slint compile (which the cargo gates DO run, unlike markup rendering).
- **Each atom owns exactly ONE mutator fn** (`pick` / `bump` / `flip`) called from BOTH the key handler and the click/AT handler, so keyboard and mouse can't diverge: the value write + the `selected`/`edited`/`toggled` callback live in one place. Integer clamping uses the GLOBAL `clamp(v, lo, hi)` which preserves `int` (same as Scrubber's `max(total-1, 0)` → int property). The two-way `current-index`/`value`/`checked` binding self-fires, but the Rust setters are idempotent so there is no feedback loop — drop-in parity with the native widgets, ZERO Rust change.
- **Tab-enabling a non-focusable button atom WITHOUT editing the atom — file-private `FocusButton` wrapper (issue 127):** When a TouchArea-based button atom (`DangerButton`/`PrimaryButton`) must become a Tab stop inside a multi-stop modal, wrap it in a file-private `component FocusButton inherits FocusScope` (same idiom as `NavBar`'s file-private `SearchField` and `Stepper`'s file-private `StepButton`). The wrapper sizes to its slotted child via `@children` and exposes a `callback pressed`. Key model: Space accepts and fires `pressed`; a catch-all `return reject` rejects every other key (including Tab, Backtab, Return, and Esc) so they bubble up to the ancestor modal `FocusScope`. This partition is compositionally safe: because the button scope rejects Return, the ancestor's "Return ⇒ cancel" binding fires even when the confirm button holds focus, so the destructive action is structurally unreachable by Enter (ConfirmDialog, issue 127).

### A modal dialog's `FocusScope` must be an ANCESTOR of the focused child control, never a sibling (issue 103, PR-B)

To catch Esc while a CHILD control holds focus, the dialog's `FocusScope` (`fs`) must WRAP the panel content so it is a genuine ANCESTOR of every control — NOT a sibling. Slint propagates a REJECTED key event strictly UP the focused item's parent chain (verified in `i-slint-core` 1.16 `window.rs`); sibling `FocusScope`s are never visited. The control atoms' inner `FocusScope`s `return reject` for keys they don't handle (←/→, ↑↓, Space), so Esc bubbles up through the ancestor `fs`, whose `key-pressed` closes on `Key.Escape` (and `reject`s everything else so other keys still reach the focused control). This was a real, gate-INVISIBLE bug: an empty SIBLING `fs` left Esc silently dead while every cargo gate stayed green — the gates do NOT exercise `.slint` key handling (see the "cargo gates do not exercise Slint markup" entry), so verify by hand.

### Focusing a child control on dialog open requires the child to EXPOSE focus — and `init` fires each open for an `if`-gated dialog (issue 103, PR-B)

A parent component cannot name an `id` declared inside a child component's body (Slint encapsulation — same scoping limit as the `root` vs `parent` and `states[]` entries), so it cannot reach into a control atom to focus its inner `FocusScope`. The atom exposes a `public function focus-control() { fs.focus(); }` instead — named `focus-control`, NOT `focus`, because `focus` is a reserved builtin on the inherited `Rectangle` and cannot be overridden. The dialog calls `direction-control.focus-control()` from `fs.init`; because the modal is `if`-gated (`if root.show-settings : SettingsDialog`), the whole subtree is reconstructed each open, so `init` fires on EVERY open (not just the first) — the correct hook to re-focus the first control each time. Mirrors the `focus-pages()`/`focus-carousel()` seam idiom.

### Backdrop-dismiss without swallowing control clicks — a scrim `TouchArea` plus a panel "absorber" declared BEFORE the content (issue 103, PR-B)

A full-area scrim `TouchArea` on the dialog root → `close-settings()` gives backdrop-click dismiss. To stop a click INSIDE the panel from also dismissing, the panel declares an empty "absorber" `TouchArea {}` as its lowest interactive layer, BEFORE the content. Slint hit-tests overlapping siblings front-to-back (later-declared = on top = tested first), so a click on a control hits the control; a click on empty panel area falls through to the absorber (which consumes it) instead of reaching the backdrop scrim behind the panel. So controls stay clickable, empty-panel clicks are inert, and only true backdrop clicks dismiss. Note: controls keep hit-test priority over the absorber's `TouchArea` because they are declared after it; the FocusScope's declaration order relative to the absorber is irrelevant to pointer routing because FocusScope does not intercept pointer events (it is keyboard-only).

### Fixed-size panel clamp on a resizable window — `min(panel, max(0px, parent − 2·gutter))` (issue 103, PR-B; content-hug since spec 2026-06-04)

A fixed-or-preferred-height panel inside a freely-resizable window clamps its height with `min(panel-h, max(0px, parent.height - 2 * gutter))`: it yields the panel height to FITTING when the window gets short (then the body scrolls), keeping a gutter on each side. The `max(0px, …)` floor guards against NEGATIVE geometry while the window is dragged smaller than `2 * gutter` mid-resize (a negative `height` would otherwise compile but render garbage). This is Marcotte's "design fluid, clamp the fixed" applied to a Slint panel. Since the 2026-06-04 visual polish, `panel-h` is the ASSEMBLED content-hug height (`padding-top + header.preferred-height + content.preferred-height + footer.height`) rather than a fixed φ constant — assembled explicitly because a `Flickable` does NOT propagate its content's preferred height (that's what lets it scroll), so a naive `self.preferred-height` would not include the body.

### A layout's literal `alignment: start/end` DISABLES child stretch factors (settings visual polish, spec 2026-06-04)

Setting a literal `alignment:` (e.g. `start`, `end`) on a `HorizontalLayout`/`VerticalLayout` makes Slint pack children at their preferred sizes and IGNORE every child's `horizontal-stretch`/`vertical-stretch` factor — including explicit stretch SPACERS. Real, gate-invisible bug: the settings footer declared `alignment: start` plus a `Rectangle { horizontal-stretch: 1; }` spacer intended to pin Close hard right; the spacer silently collapsed and Close rendered glued to Shortcuts (every cargo gate green — the gates do not exercise markup rendering). Fix: omit `alignment:` (default `stretch`) and let stretch factors own the slack. Corollary for mixed rows: to right-align a COMPACT child without killing a sibling FILL child's stretch, inject a conditional leading spacer (`if cond : Rectangle { horizontal-stretch: 1; }`) instead of switching the layout's alignment — `SettingRow`'s `trailing` variant is the canonical example.

### Slint `vertical-alignment: center` centers the font's METRIC line box, not the glyph mass — descender-less labels sit ~1px high in tight capsules (settings segmented, fix 2026-06-04)

`Text`'s `vertical-alignment: center` centers the font's full line box (ascent + descent). A UI label with NO descenders ("Standalone", "Right to Left", digits) only occupies the cap-height band, so its visible glyph mass sits about half-a-descent (~1px at `font-label` 12px) ABOVE the box's visual center — inside a tight pill the under-text gap reads visibly wider than the over-text gap (measured 7px above vs 9px below the caps). HARNESS: add a small downward `y:` nudge (≈ descent/2 — 1px at 12px) on the label so the GLYPHS, not the em box, read centered; this is the optical centering Apple applies to control labels. When the label is a LAYOUT child (the layout owns its `y`), express the same nudge as an asymmetric vertical-padding split instead — PrimaryButton splits its 16px vertical padding 9 top / 7 bottom. Verify by pixel-measuring a screenshot (the cargo gates render nothing); only apply where a label sits inside a visually tight capsule — loose contexts hide the asymmetry.

### An element placed OUTSIDE a layout defaults to its PARENT's size — a manually offset row needs an explicit `height` (settings footer, fix 2026-06-04)

A non-layout child (here `footer-row`, a `HorizontalLayout` positioned with `x`/`y` inside the footer `Rectangle`) defaults `width`/`height` to its PARENT's — so the row silently filled the whole footer block, its `y: space-md` offset pushed its bottom edge `space-md` PAST the panel's, and the layout's cross-axis stretch inflated the Close button to the full block height. Gate-invisible (rendering only); latent since the footer was built, it became VISIBLE when the visual-polish PR moved the panel's bottom inset into the footer block, putting the block's bottom edge exactly on the panel's. HARNESS: any element positioned manually (given `x`/`y`) inside a plain `Rectangle` must also pin its size — `height: self.preferred-height` for a content-sized row — never rely on the parent-size default when an offset is in play.

### A layout stretches children across its CROSS axis past `min-height` — a fixed-proportion atom pins `width`/`height`, not `min-*` (settings toggle, fix 2026-06-04)

`min-width`/`min-height` are FLOORS, not sizes: a `HorizontalLayout` stretches each child across its cross axis up to the child's `max-*` (unbounded by default), and `vertical-stretch: 0` does NOT opt out — stretch factors only govern slack distribution along the layout's own axis. The Toggle declared `min-height: 30px` + `vertical-stretch: 0`, and the SettingRow slot still inflated the track to the 34 px row height, silently breaking the φ track ratio and bloating the knob (all gates green — rendering only). HARNESS: an atom with a DESIGNED proportion (the φ toggle track) pins explicit `width:`/`height:` bindings, which a layout cannot override; reserve `min-*` for genuinely flexible elements. Segmented/Stepper already pinned `height:` — Toggle was the odd one out.

### Glass scroll body — `Flickable` with a self-drawn indicator, NOT std `ScrollView` (issue 103, PR-B)

For a scrollable region inside the glass panel, use a `Flickable` — NOT the std-widgets `ScrollView`, whose native scrollbar paints a light palette that fights the dark glass (same root cause as the std-widgets-light-palette entry). Set `viewport-height` to the content's `preferred-height` so it scrolls ONLY on overflow. Draw the indicator yourself: a thin `track-prog` rail with an `accent` thumb, shown only on overflow via `visible: viewport-height > height` (NEVER `opacity` — the HiDPI offscreen-blur rule). Compute the thumb's height/`y` in the SAME coordinate space as the `Flickable` (derive the indicator's `y` from the enclosing layout offsets, not a bare `body.y`, so it stays aligned if the content layout is ever offset within the scope). Put `clip: true` on the rail so during macOS elastic OVERSCROLL — where `viewport-y` can briefly go POSITIVE, making the thumb `y` negative — the thumb stays inside the rail bounds instead of drawing above it.

### Glass panel = NavBar's 4-layer fake-glass idiom with layer 1 promoted to a top-sheen gradient (issue 103, PR-B)

The settings panel reuses NavBar's four-layer fake-glass idiom (Slint 1.x has no backdrop-blur): a solid fill + a 1px rim (`glass-border`) + a 1px top inner highlight (`glass-highlight`, inset horizontally by ~half the radius to stay inside the rounded corners) + ONE drop-shadow. The only delta: layer 1 is PROMOTED from a solid fill to a top-sheen `@linear-gradient(180deg, glass-sheen-top 0%, glass-fill 46%)` — a FILL gradient, not an `opacity` layer (ties to the HiDPI opacity-blur rule: an opacity layer would blur the panel's text/SVG). It is ONE fake-glass object: no nested glass, no second shadow (the single-`Rectangle`-one-drop-shadow constraint).

### `SettingRow` molecule: one seam + one right rail; the ATOM's stretch factor decides fill vs trailing — `@children` once per component (issue 103, PR-B; right rail since spec 2026-06-04)

The original L1 defect was a ragged left edge from per-row `HorizontalBox` + double-sided stretch (each row negotiating its own width). `SettingRow` (`components/SettingRow.slint`) owns the alignment doctrine: a fixed-width label column (`settings-label-col`, 132px) at `x: 0`, and an `@children` control slot spanning ONE shared vertical seam (`settings-control-x`) to the shared RIGHT RAIL (the slot's right edge = the body's right padding edge). The rule is one sentence: **every control ENDS at the right rail; fill controls also START at the seam.** Whether a control fills is decided by the ATOM's own stretch factor — `Segmented` sets `horizontal-stretch: 1` (equal-width cells fill seam→rail), `Stepper`/`Toggle` pin `horizontal-stretch: 0` (and the fixed-width `Dropdown`) and their rows opt into `trailing: true`, which injects a conditional leading spacer that pushes the compact atom onto the rail (see the alignment-kills-stretch entry above for why the slot layout must NOT use a literal `alignment:`). The two settings steppers equalize width via a 3-digit `min-width` floor on the value text so the rail reads as a column, not stairs. The label is `no-wrap` + `overflow: clip` (not elide) so the seam never drifts, in `text-mid` (NOT accent — accent stays interactive/selected-only). `@children` is allowed exactly ONCE per Slint component, which is why the row exposes a single control slot. It carries a `stacked` (L4 i18n/RTL) escape-hatch property for a future label-above-control pivot, but that is currently a DOCUMENTED INCOMPLETE STUB — the row height is not yet adapted (a 30px control overflows the ~17px half-row), so do NOT set `stacked: true` at any call site until the adaptive-height layout is finished.

### Returning focus INTO an `if`-gated dialog from outside — bump an epoch property the dialog observes with `changed` (issue 104, PR-C)

A parent cannot reach an `id` inside an `if`-gated child (the `root` vs `parent` / `focus-control()` encapsulation limit), so it cannot call `fs.focus()` on a still-mounted `SettingsDialog` after a stacked overlay closes. The seam: the parent holds a PRIVATE `property <int> settings-focus-epoch` and a `public function focus-settings() { settings-focus-epoch += 1; }`; the dialog declares `in property <int> focus-epoch` bound to that property plus a root-level `changed focus-epoch => { fs.focus(); }`. Bumping the int from Rust (`ui.invoke_focus_settings()`) drives the dialog's own `FocusScope` so Esc/Tab are live again on return. This works BECAUSE Slint `changed` handlers do NOT fire on the initial binding/mount — only on subsequent value changes — so the OPEN sequence stays owned entirely by `init` (the `focus-control()`/`init`-fires-each-open entry) and the epoch handler fires ONLY on a genuine re-focus request. Rust side: guard the invoke with `if ui.get_show_settings()` so a future settings-less entry point to the overlay can't silently no-op the focus call (it returns focus to nothing). Same class as the `focus-pages()`/`focus-carousel()` seam — focus is an invariant the code restores explicitly at every transition.

### Modal-over-modal: a second overlay stacked on a still-mounted dialog must TRAP every key so focus can't leak underneath (issue 104, PR-C)

`ShortcutsOverlay` opens OVER the still-mounted `SettingsDialog` — both `show-settings` AND `show-shortcuts` are true at once. Extends the modal entries above with three load-bearing rules. (1) Stacking: the overlay is the LAST `if`-gated `Window` child (last-declared = topmost paint), and BOTH scrims draw, so the dialog dims a SECOND time behind it — intended layering that signals "modal over modal". (2) Key trap: the dialog underneath stays keyboard-operable, so the overlay's ancestor `FocusScope` (the FocusScope-ancestor rule) grabs focus on `init` and its `key-pressed` returns `accept` for EVERY key — Esc closes, arrows/PageUp/PageDown scroll the overlay's own `Flickable`, and a catch-all `accept` swallows everything else (notably Tab) so focus can NEVER reach a settings control hidden behind the dim. (3) Guards: every background key-guard must list the new flag — the page `FocusScope` (`if (show-settings || show-guide || show-shortcuts) return reject`) and the carousel `modal-open` (`show-settings || show-guide || show-shortcuts`) — or keys leak behind BOTH modals. Closing returns focus to the dialog underneath via the epoch seam above, NOT to the screen behind. Dismiss triad mirrors the dialog's: Esc, backdrop-click, Close button.

### Modal Tab containment — a multi-stop modal must self-rotate Tab; a catch-all `accept` kills window-level Tab navigation (ConfirmDialog, issue 127)

A modal with TWO OR MORE focus stops cannot rely on Slint's window-level Tab navigation to cycle between them. Slint's key dispatch walks a rejected key strictly UP the focused item's parent chain (i-slint-core 1.16.1 `window.rs`); only when that upward walk reaches the top WITHOUT returning `EventAccepted` does the window-level `focus_next_item`/`focus_previous_item` fallback fire (`window.rs:885–905`). A catch-all `return accept` in the ancestor `FocusScope` — correct for a single-stop modal like `ShortcutsOverlay` — makes that upward walk return `EventAccepted` before the window fallback ever runs, so Tab is silently swallowed and any stop past the first becomes pointer-only. This failure is **gate-invisible**: the cargo gates do not exercise `.slint` key handling.

The failure boundary is the stop count:

- **One focus stop** (e.g. `ShortcutsOverlay` where `fs` holds all focus itself): catch-all `accept` is safe — there is nowhere else to go.
- **Two or more stops** (e.g. `ConfirmDialog`'s Cancel and Confirm): the ancestor `FocusScope` must handle Tab and Backtab explicitly, rotating among the stops IN-TRAP, and STILL return `accept` so no key leaks to the live content behind the modal.

The explicit rotation for two stops: `cancel-scope.has-focus ? confirm-scope.focus() : cancel-scope.focus()`, then `return accept`. Rejecting Tab to let the window fallback run is NOT safe when a live, focusable backdrop (e.g. the carousel's `FocusScope` on the library screen) exists behind the modal — window navigation would carry focus OUT of the dialog.

Backtab arrives as either `Key.Backtab` or `Key.Tab` with a shift modifier depending on the backend (`window.rs:897–899`). For a symmetric two-stop modal the toggle is direction-agnostic (both forward and backward are the same swap), so matching `event.text == Key.Tab || event.text == Key.Backtab` in one arm is sufficient — modifier inspection is not needed.

### Dialog save failures log `tracing::error!` (matching the other save sites, NOT `warn!`) AND surface to the status bar on close (`ui.set_status_text`)

A `tracing` line alone is invisible in a GUI run (`RUST_LOG` usually unset) — same rationale as surfacing the skipped count. The guide-dismiss save failure degrades gracefully (the guide simply re-shows next launch; `seen_guide` is also saved on exit) — intentional non-fatal.

Routing the outcome to a status property is only half the fix: a bound, VISIBLE widget must exist on the screen where the action RUNS (PR-L). The shared `status-text` is shown by a Viewer-screen `Text` gated `visible: screen == 1`; a Library-screen action (Add Files/Folder on screen 0) would set the property silently with nothing on screen. PR-L therefore mounted a second `status-text`-bound `Text` gated `visible: screen == 0`. Rule: route user-facing outcomes to a widget visible in the CURRENT screen, not just to any bound property.

### `refresh()` OVERWRITES `status-text` — surface notices AFTER it, and COMPOSE (append) when several can co-occur (PR-La)

`refresh()` pushes the base spread/status string into `status-text`, so any load/save error notice set BEFORE it is silently clobbered. Set such notices AFTER `refresh()` (the startup load-failure notice is set after the *initial* refresh; the open-path save-failure notices after the open-path refresh). When more than one notice can fire from a single action — on the open path: skipped entries + settings-save failure + library/page-count-save failure — COMPOSE them by APPENDING to the current status (`{base} \u{2014} {detail}`, em-dash), never replacing, so an earlier notice isn't lost. `app::OpenBookUseCase::run` (the former `open_and_present`, see the use-case-object bullet above) decides WHICH notices appear via the pure `status_notices(...)` fn and iterates its `Vec<String>`, appending each onto the current status (the old single `append_status` closure is gone); the save outcomes are captured into locals (`settings_save`, `library_save`) BEFORE `refresh` and surfaced after it, in a fixed order (skipped, then settings, then library). (Extends the PR-L "route to a visible widget" bullet above with the refresh-clobber + compose-don't-replace angle.)

### Runtime state is the SINGLE source of truth for the four display modes; `Settings` mirrors them ONLY via `reconcile_settings`, just before each save (PR-D / issue #32)

`ViewerState` owns `reading_direction`/`spread_mode`/`cover_mode`; `ViewportState` owns `fit_mode`. `reconcile_settings(&ViewerState, &ViewportState, &mut Settings)` (a pure fn in `main.rs`) copies those four into `Settings` immediately before EACH `save()` — exit, settings-dialog close, and the open-time save (INSIDE the `if track_recent_files` gate in `app::OpenBookUseCase::run`, the only save on that path). Mode-mutation sites (D/R/C/`f` keys + the dialog setters) now ONLY mutate runtime state + `refresh`; the ~9 per-mutation `settings.borrow_mut().X = …` mirror lines are GONE, killing the "a new mutation site forgets to mirror → setting silently not persisted" bug class (neither types nor tests caught it before). The guide-dismiss save writes only `seen_guide` and intentionally SKIPS reconcile (not a runtime-mirrored field). EXCEPTION: `cache_size`/`preload_pages`/`track_recent_files` keep `Settings` as their source (one-way `Settings → ViewerState` via `set_cache_config` — see that bullet above); they are NOT reconciled back. `on_open_settings` reads the dialog's initial mode values from the RUNTIME (`state`/`viewport`), never `Settings`, so a lagging mirror can't make the dialog show a stale value.

### Per-book view overrides: write-back-at-leave-point + screen-scoped dialog routing (per-book view overrides)

Builds DIRECTLY on the PR-D reconcile entry above: the four view modes are now PER-BOOK overrides with the global `Settings` as the fallback, so the runtime↔persistence wiring grew a second, screen-scoped target. (See the value-object pair and the write-direction-audit gotcha above; this entry is the runtime-flow how-to.)

- **Write back runtime → per-book override at EVERY leave point.** `write_back_view_override(&state, &viewport, &library)` snapshots the open book's four runtime modes into its `ViewOverride` and saves the library — fired at nav-away (↑ to Library), open-another (top of `OpenBookUseCase::run`), app exit, and Viewer settings-dialog close. WHY at every leave point: a bare keyboard toggle (D/R/C/`f`) must persist for THAT book without the user opening any dialog — exactly mirroring how PR-D made bare toggles persist into global `Settings` via the save-on-exit reconcile, and how PR-R writes the reading position back at every leave point (`write_back_view_override` sits right beside `write_back_position` at each of those sites). It is a no-op when no book is open (`open_file()` is `None`).
- **Gate the GLOBAL reconcile on "no book open."** The exit-path `reconcile_settings` now runs only `if state.borrow().open_file().is_none()` — otherwise the open book's per-book modes (just written to its override) would clobber the global defaults. This is the exit half of the write-direction invariant in the gotcha entry above.
- **The SAME settings dialog edits different targets by SCREEN.** There is ONE `SettingsDialog`; `ui.get_screen()` selects scope on close — `0` (Library) → reconcile into GLOBAL `Settings`; `1` (Viewer) → `write_back_view_override` into the current book. Seeding the dialog when opened over the Library requires mirroring GLOBAL → runtime FIRST (`apply_global_view_to_runtime(&settings, &state, &viewport)`), so the runtime carries the global defaults while that dialog is open; closing it on the Library screen runs the inverse `reconcile_settings`. (On the Viewer screen the runtime already holds the open book's resolved modes, so no pre-seed is needed.)
- **Re-seed on RETURN to a book.** Opening/resuming a book applies its `ViewOverride::resolve(&Settings)` via `ViewerState::apply_resolved_view(resolved)` + `ViewportState::set_fit(resolved.fit_mode)` AFTER the source is set — which is exactly why the runtime is NOT reset at open time and why the open-path reconcile is the clobber trap (see the gotcha entry).

TRADE-OFF worth stating: `write_back_view_override` pins ALL FOUR fields to `Some` after the first leave from a book, so a book that has been opened once carries a FULL override (it stops tracking later global changes for those modes). The Viewer settings dialog's "Reset to global" button clears the book back to `ViewOverride::none()` (all-`None` → inherit again). A finer-grained "only persist the fields the user actually changed" was deliberately NOT built — the `Option<Enum>`-per-field shape leaves room for it, but the leave-point snapshot writes the whole tuple. See ADR-0007 for why uniform full-override write-back was chosen over change-tracking.

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

### PopupWindow dropdown menu: lowered to the window root; pad the popup for its shadow; keyboard cycles inline (i18n PR)

The settings dialog's scroll body is a clipping `Flickable`, so an in-tree dropdown menu would be clipped at the viewport edge. `Dropdown.slint` (the repo's first `PopupWindow`) sidesteps this: the Slint compiler lowers a `PopupWindow` to the window root, so the open menu can never be clipped by an ancestor. Three harness points. (1) A `PopupWindow` clips to its OWN bounds too — pad the popup surface by the menu shadow's bleed (`shadow-pad`) and inset the visible panel by that pad, or the drop shadow is sliced off at the popup edge. (2) Don't put a `FocusScope` inside the popup (popup focus is a known Slint weak spot); the BUTTON keeps focus and Up/Down cycle the selection inline (Segmented-style), so the control stays fully keyboard-operable without the menu ever needing focus, while Space/Enter open the menu for pointer-style selection. (3) The default `close-policy` (close-on-click) dismisses on item click, outside click, AND Esc for free — and because the button's FocusScope `reject`s Esc while the menu is closed, a second Esc still walks up to the dialog's ancestor FocusScope and closes the dialog (HIG two-step dismiss).

### gettext is GONE; Fluent is the sole catalog — fix forward, and adding a language is a compile/test-gated chain (Fluent i18n PR-4, #115)

As of PR-4 (issue #115) the gettext path was excised entirely: the `build.rs` `.with_bundled_translations` / `.with_default_translation_context` flags, the `translations/ja/LC_MESSAGES/gashuu.po` tree, `select_ui_language` + `slint::select_bundled_translation`, the OUT_DIR canary `bundled_translations_compiled_into_generated_code`, and the `.po`-reading test `ftl_static_channel_covers_every_po_msgid` are all deleted. Fluent (i18n-embed + `fl!()`) is now the ONLY i18n system. This is a one-way door: there is no `@tr()` / `.po` fallback to revert onto, so a regression in the Fluent path must be **fixed forward**, not rolled back.

HISTORICAL (the trap worth remembering): the deleted gettext path defaulted Slint's translation context to the *enclosing component name*, sent as the gettext `msgctxt`; a flat `.po` carrying no `msgctxt` entries then matched NOTHING — every string silently fell back to English with the build still green (`build.rs` had to set `.with_default_translation_context(DefaultTranslationContext::None)` to collapse to one flat msgid namespace). Preserved here only because a "total, invisible, gates-green" failure is the kind worth never repeating.

**Adding a new language is a chain where every step's gate is compile-time or test-time — no step can be silently skipped:**

1. Add the variant to `pub enum Language` in `gashuu-core/src/settings.rs` (today `En` / `Ja`).
2. `langid_for(lang: Language)` in `src/i18n/loader.rs` now FAILS TO COMPILE — it is an exhaustive `match` with no wildcard arm, so the new variant forces you to wire its BCP-47 `langid!`.
3. Add `crates/gashuu/i18n/<lang>/gashuu.ftl`. The `all_ftl_ids_present_in_every_locale` test (in `src/i18n/mod.rs`) then FAILS until the new file carries every message ID present in the others — it forces full translation coverage, not a partial stub.
4. Extend the Slint adapters in `src/enum_adapters.rs`: `language_to_index` is an exhaustive `match` on `Language` with no wildcard — a new variant fails to compile there. Its reverse `index_to_language` matches on `i: i32` with a `_ => Language::En` wildcard, so a new variant does NOT cause a compile error; instead the round-trip test `language_index_round_trips` (in `enum_adapters.rs`) catches any missing reverse mapping at test time. Also add the option to the language `Dropdown` model in `ui/SettingsDialog.slint` (`model: ["English", "日本語"]` — option labels are deliberately each in their OWN tongue, not translated).

For the rest of the Fluent harness do NOT re-derive it here — the full machinery lives in the entries below: the `Strings`-global push chokepoint ("The `Strings`-global push: serve Fluent strings to Slint through one Rust chokepoint"), the single `Localizer::apply()` write point and its two-tier safety (`fl!()` compile gate vs fallback + the exhaustive `langid_for` match + the `all_ftl_ids_present_in_every_locale` test), the `{" "}` leading-whitespace literal and the verb-final named-args word-order rule ("Fluent catalog authoring gotchas"), and the test oracle/parity floors ("i18n test harness: the legacy catalog as a byte oracle").

### Removing a design token: sweep by NAME, VALUE, and CONCEPT to a fixed point (settings visual polish, spec 2026-06-04)

When a token is deleted (e.g. the fixed `Theme.settings-h` φ panel-height constant removed in favour of content-hug), a single grep pass on the token NAME alone does not converge — reviewers found stragglers that used the LITERAL VALUE (`583`) or CONCEPT PHRASES (`"golden-ratio panel"`, `"fixed-height"`, `"keeps its fixed φ size"`, `"{elevation.float} shadow"`) rather than the token identifier. Run ALL THREE patterns up front, over `crates/`, `docs/`, AND `DESIGN.md` simultaneously, before the first commit. DESIGN.md states each fact in up to THREE places (the frontmatter token block, prose bullets, and per-component sections), and other docs (`conventions.md`, `architecture.md`, `patterns.md`) may restate it — every restatement is a sweep target. When editing DESIGN.md frontmatter formulas, do a DANGLING-SYMBOL CHECK: verify that every `{ns.key}` reference in a formula is defined somewhere in the frontmatter — both a missing `controlHeight` key and a `{colors.shadow-popover}` reference introduced before its definition were caught in this PR. A cheap mechanical pre-commit grep for all swept patterns confirms the fixed point and catches a fix that itself introduces a new stale reference.

### Fluent loader (i18n-embed 0.16): `load_languages` REPLACES, auto-appends fallback, and resets bundle config (Fluent i18n PR-1, #112)

This is the *Fluent* side of i18n; the gettext/Slint `@tr()` side (msgctxt trap, OUT_DIR canary) is HISTORICAL — see "gettext is GONE; Fluent is the sole catalog" above and do NOT re-derive it here. The `messages.rs` exhaustive-match catalog was deleted in PR-3 (#114); the compile-time gate equivalent is now `langid_for(Language)` in `loader.rs`. The two catalog systems coexisted through PRs 1–3; the gettext path was excised in PR-4 (#115), leaving Fluent as the sole catalog. ADR-0008 has the staging.

`FluentLanguageLoader::load_languages(&assets, &[requested])` is a TOTAL replace, not a layering: it AUTO-APPENDS the fallback language (`en`, from `i18n.toml`) when absent, then atomically swaps ALL loader state via `ArcSwap`. Three consequences, each verified against the vendored 0.16 source (`fluent.rs:543-582`) and pinned by tests:

1. **No preceding `load_fallback_language`.** Its effect is immediately discarded by the next `load_languages` swap — calling it first is dead motion. (An early draft had it; removed in `b6bb0eb`. `Localizer::new` carries a comment so it doesn't come back.)
2. **`current_languages()` reports ONLY the caller-supplied list** (`["ja"]`), never the auto-appended fallback — the loader stores `language_ids`, not the extended `load_language_ids`. So the fallback is *structurally* guaranteed but **behaviorally UNOBSERVABLE** while catalogs are in ID lockstep (`all_ftl_ids_present_in_every_locale`): NO `en`-fallback resolution event can fire, so don't claim a test "proves the fallback works" — a real fallback-resolution test belongs to the PR where a translation can actually be missing.
3. **`set_use_isolating(false)` is bundle config that the swap RESETS.** FluentBundle defaults to wrapping placeables in FSI/PDI bidi marks (U+2068/U+2069); leaving them on inserts invisible codepoints that break byte-identity with legacy strings. The call has NO effect before `load_languages` and must be RE-APPLIED after EVERY `load_languages` (both `new` and `switch`). `switch_swaps_languages_and_keeps_fallback` asserts no FSI/PDI survive a swap.

On a missing message, `loader.get*` returns the literal `No localization for id: "<id>"` and emits a `log::error!` — that is the **`log` crate, not `tracing`**; the app logs via `tracing`, so the signal is dropped unless a `tracing-log` bridge is wired before PR-2 (#113) starts consuming Fluent output. Note this when wiring consumption.

**Load-failure policy is deliberately LOUD and asymmetric.** `Localizer::new`/`switch` `panic!` on a load error: assets are compile-time-embedded (`RustEmbed`) and `langid_for` is an exhaustive `match`, so a failure is a programmer error, not a runtime condition. This intentionally diverged from the now-deleted `select_ui_language`'s never-fatal `tracing::warn` (the gettext path, excised in PR-4 #115), and the asymmetry is documented at the `switch` call site in `main.rs` — the rationale is the repo's history of a silent gettext all-miss (the msgctxt trap above): for the catalog we control end-to-end, fail fast.

### Fluent catalog authoring gotchas (verified by exact-equality tests vs the former `messages.rs` byte oracle)

The Fluent `.ftl` values were pinned byte-identical to the legacy `messages.rs` arms by exact-equality tests (`shortcuts_help_matches_legacy_catalog_byte_for_byte`, `already_in_library_preserves_em_dash`, `skipped_detail_preserves_leading_space`). `messages.rs` was deleted in PR-3 (#114); the byte pins remain as standalone FTL-content tests. Authoring traps surfaced empirically:

- **Leading whitespace in a value is trimmed.** To keep a historical leading space (e.g. ` (zip-slip or oversized)`), wrap it in a string-literal placeable: `{" "}(...)`. (ADR-0008 §Consequences notes this; the test above pins the exact byte value.)
- **Multiline block values strip the COMMON indentation, but KEEP interior blank lines.** A block authored at 4sp (headers) / 6sp (body) delivers 0sp / 2sp; blank lines *inside* the block survive as real newlines with no `{""}` encoding needed. This is why a `shortcuts-help` block reproduces the legacy 2-space-indented text exactly.
- **The parser does NOT error on a duplicate message ID in one file** — Fluent silently last-wins at runtime. A guard is explicit: collect IDs into a `Vec` (source order) and assert `Vec::len() == Set::len()` per file (folded into `all_ftl_ids_present_in_every_locale`); a `Set` alone would absorb the duplicate.
- **Named args are word-order-safe**, which the old positional gettext `{}` concat could not express: `{ $label }を減らす` (ja, verb-final) vs `Decrease { $label }` (en) are the SAME message with the placeable in a different position.

### i18n test harness: the legacy catalog as a byte oracle; floors and loud catch-alls against vacuous passes (Fluent i18n PR-1, #112)

While both i18n systems coexisted (PRs 1–2), the legacy catalogs were LIVE ORACLES for migration fidelity — richer than shape tests. `messages.rs` was deleted in PR-3 (#114); the byte pins remain as standalone FTL-content tests (see "Test guarantee migration discipline" below):

- **`messages.rs` as a byte-identity oracle (historical).** Exact-equality tests against the `msg_*` arms pin the migration far better than line-count/shape tests (a shape test passes on a reworded translation). Keep both kinds: a line-count test localizes a diff, the byte test catches the content drift. After deletion the same byte pins live in `i18n::dynamic::tests`.
- **(HISTORICAL — test deleted in PR-4 #115 with the `.po` tree) A coverage test that parses an external oracle MUST carry a vacuous-pass FLOOR.** `ftl_static_channel_covers_every_po_msgid` asserted `.po`-msgids-⊆-`en.ftl`-values; it parsed the `.po` by line-prefix and asserted `po_msgids.len() >= 50` BEFORE the subset check — a reformat of the `.po` (e.g. multi-line msgids) would otherwise have broken the parser, found zero msgids, and passed a check over an empty set. The generalizable lesson outlives the test: any subset/coverage check needs a count floor or it false-passes on an empty oracle.
- **AST-walk catch-alls in test reconstruction PANIC, never return empty.** The `_ =>`/`other =>` arm when reconstructing an `.ftl` value from its AST `panic!`s with "extend this match arm" — a silent truncation (returning `""` for an unhandled placeable kind) would false-pass the coverage test.
- **Cross-locale `$arg`-set parity via the AST** (`message_arguments_match_across_locales`: same `$var` name set per shared ID in `en` and `ja`) closes a gap that neither `fl!()` (fallback-only compile check, can't see `ja`) nor ID-parity (`all_ftl_ids_present_in_every_locale`, IDs only, not args) covers — a `$lable`/`$label` typo in one locale would otherwise surface only as a runtime log + malformed string in PR-3.
- **The exhaustive `langid_for(Language)` match (no wildcard)** is the compile-time gate that replaced `messages.rs`'s exhaustive-match safety: a new `Language` variant fails compilation until a catalog `langid` is wired.
- **(PR-2, #113) An end-to-end composition pin doubles as cross-file default reconciliation.** `composed_stepper_labels_match_apply_composition` reproduces `apply()`'s EXACT two-step Stepper a11y compose — resolve `settings-cache-a11y` FROM the catalog, then feed it as the `label` named arg into `stepper-decrease`. This catches what the PR-1 `parameterized_messages_embed_arguments` test cannot: that test hardcodes the label literal and only asserts `starts_with`/`ends_with`, so it is blind to a label CROSS-WIRE (feeding `settings-cache-label` "Cache size (pages)" instead of `settings-cache-a11y` "Cache size in pages") and, for Ja, to a verb-final word-ORDER regression (`減らす（{ $label }）` still passes an `ends_with`). The pin asserts BYTE-EXACT composed strings for both locales. Second duty: its four English literals are deliberately the SAME strings as `Strings.slint`'s composed `stepper-*` defaults — so an `en.ftl` edit that orphans the `.slint` defaults (the stale-En blind spot the literal-default insurance creates) fails this test instead of shipping silently.

### Don't add a `pub(crate)` getter for a future consumer — it trips `dead_code` now; add it in the PR that has the real consumer (Fluent i18n PR-1, #112 / resolved PR-3, #114)

`Localizer` wraps a private `loader` field. A `pub(crate) fn loader()` getter intended for PR-2's `Strings`-push call sites would be flagged `dead_code` under `--all-targets -D warnings` TODAY: in a *binary* crate `pub(crate)` is not an API surface, and a getter used ONLY by same-crate `#[cfg(test)]` code does not count as a live use. The right move is to NOT introduce the getter at all until the real consumer lands. The same-module `#[cfg(test)] mod tests` reaches the private field directly (`localizer.loader.get(...)`), so no getter is needed for the tests. PR-2 vindicated this: `apply()` (the real consumer) landed inside `i18n/mod.rs` and reads `self.loader` directly. PR-3 (#114) finally added the `pub(crate) fn loader()` getter because `i18n/dynamic.rs` is in a SIBLING file (`i18n/dynamic.rs` vs `i18n/mod.rs`) — no longer the same module — and is the real cross-file consumer; the getter now has a non-test caller and is gate-clean.

### The `Strings`-global push: serve Fluent strings to Slint through one Rust chokepoint (Fluent i18n PR-2, #113)

Slint's `@tr()` cannot consume Fluent (see ADR-0008). The sanctioned bridge: a Slint `export global Strings` (`ui/Strings.slint`) of `in property <string>` slots (66 here) with English-literal defaults, ALL written from Rust by a single chokepoint `Localizer::apply(&self, ui: &ViewerWindow)` — every `fl!()` call in the crate lives in `apply()`, called once at boot (after `Localizer::new`) and again after every `switch()`. `.slint` bindings read `Strings.<prop>` instead of `@tr()`. Harness, each verified against code:

1. **Re-export the global from the BUILD ENTRY POINT or Rust gets no accessor.** Slint only generates `ui.global::<Strings>()` for globals exported from the compiled entry file. `ViewerWindow.slint` does both `import { Strings } from "Strings.slint";` AND `export { Strings }`; the bare `import` alone is not enough. (Mirrors the `Theme` global's wiring.)
2. **`in` (not `in-out`) suffices for Rust-side `set_*` on Slint 1.16.1** — resolved empirically; this was an open question entering PR-2. Rust `set_<prop>()` setters are generated for plain `in` properties, so the global stays write-only-from-Rust / read-only-from-`.slint`, which is exactly the data direction.
3. **English literal defaults are insurance, not decoration.** If a frame paints before the first `apply()`, or a wiring regression drops a push, the label shows STALE-ENGLISH rather than blank — visible degradation instead of an invisible empty string. That same property makes the defaults invisible in the En locale (stale-En == correct-En), which is WHY the composed defaults are test-pinned — see the i18n-test-harness entry below.
4. **Property names == Fluent message IDs** (kebab in `.slint` ↔ snake in generated Rust setters), so `grep navbar-search-a11y` crosses the `.ftl` → `Strings.slint` → `apply()` boundary in one query.
5. **A sequential 66-setter swap is visually atomic.** Slint batches property changes and repaints them together before the next frame, so pushing 66 setters one-by-one in `apply()` cannot produce a half-translated frame — no redraw hack, no double-buffer needed.

### Word-order-safe composed a11y labels: compose in Rust via Fluent named args, bind a plain label prop on the component (Fluent i18n PR-2, #113)

A parameterized a11y string ("Decrease {label}") must be composed in RUST via a Fluent named arg (`fl!(…, "stepper-decrease", label = …)`), never assembled by Slint-side string concatenation — the *why* (verb-final Japanese `{ $label }を減らす` vs English prefix order) is already in conventions.md ("Fluent catalog message IDs") and the PR-1 "Fluent catalog authoring gotchas" entry; do not restate it. The PR-2 half is the COMPONENT shape: `Stepper.slint` exposes plain `in property <string> a11y-decrease-label` / `a11y-increase-label` (no `{...}` template, no concat), and `SettingsDialog.slint` binds them to PRE-COMPOSED `Strings.stepper-decrease-cache` / `…-preload` properties that `apply()` filled with the two-step `fl!()` compose. So the component never sees the verb or the noun separately — it receives one finished string per locale.

### The gettext bundler is keyed by LIVE `@tr()` call-sites, not by the `.po` — so a guard that asserts on the bundled BYPRODUCT dies when `@tr()` is removed (Fluent i18n PR-2, #113)

Slint’s `with_bundled_translations` compiled ONLY the `.po` entries whose msgid matched a live `@tr()` source string at compile time — the `.po` file alone contributed nothing. PR-2 removed every `@tr()` from `.slint` (zero remained, an issue-113 acceptance criterion), leaving the `.po` + `build.rs` `with_bundled_translations` flag in place as an inert rollback surface. Consequence for the PR-1 OUT_DIR canary (`bundled_translations_compiled_into_generated_code`): with no `@tr()` source strings the gettext bundler matched nothing, so NO Japanese catalog *text* (`見開き`) reached generated code. The canary was therefore half-retired: it was narrowed to assert ONLY that the `“ja”` locale slot was registered (failing loudly if the build flags were ripped out prematurely), and the “the ja catalog actually SAYS 見開き” guarantee moved to the loader-level test `i18n::tests::ja_catalog_pins_spread_vocabulary`. PR-4 (#115) then deleted the rollback surface — the `.po` tree, the `build.rs` flags, and the half-retired canary — completely. The guarantee survived only because it had already been re-anchored to the Fluent loader before that deletion. Generalizable lesson: a guard that asserts on a BYPRODUCT of consumption (here, bundled gettext text) breaks when consumption moves — re-anchor the guarantee to the new consumer rather than weakening or deleting the guard wholesale.

### Neutral content structs: decouple domain logic from i18n formatting (Fluent i18n PR-3, #114)

`viewer_state.rs` and `app.rs` contain zero `crate::i18n` imports. Instead each exposes a "neutral" content struct that carries formatting inputs without locale: `ViewerState::status_content() -> StatusContent` (holds `StatusKind`, the page-range `String`, `SpreadMode`, `ReadingDirection`) and `OpenBookUseCase::run()` returns `OpenOutcome::Success(NoticesContent)` (holds skip count, `SkippedDetail`, optional pre-captured error strings). The formatting boundary lives entirely in `i18n::dynamic`: `format_status(loader, &StatusContent)` and `format_notices(loader, &NoticesContent)`. The gain: domain tests exercise the struct fields without constructing a `Localizer`; i18n tests exercise formatting functions with fixed structs, keeping the two concerns orthogonal. Applicable to any Rust project where UI formatting needs to be separated from domain state.

### `OpenOutcome` pattern: defer i18n and UI wiring to the call site (Fluent i18n PR-3, #114)

When a use-case `run()` must stay i18n-free, return an `enum OpenOutcome { Error(String), Success(Data) }`. The `Error(String)` payload pre-captures `format!("{e}")` at the earliest point inside the module boundary so `CoreError` never leaks out; the `String` is already display-ready and does not need a type alias outside the module. `main.rs::finalize_open` then wraps it in `i18n::dynamic::open_error_str(loader, &e_str)`. This keeps the error's source-module from needing a `crate::i18n` import while still giving the caller a formatted, localized string. The pattern generalizes: any fallible use case that would otherwise need a locale parameter can return `Error(String)` + `Success(NeutralData)` and let the immediate caller supply the locale.

### `finalize_open` helper: extract the shared `OpenOutcome` match when it appears at N call sites (Fluent i18n PR-3, #114)

`main.rs` has three open handlers (`on_open_folder`, `on_open_archive`, `on_carousel_open`); all three do the identical `match outcome { Error → set status, Success → refresh + append notices }`. Extract `fn finalize_open(ui, state, viewport, localizer, outcome)` in `main.rs` — NOT in `app.rs` — so `app.rs` stays free of `crate::i18n`. The helper belongs to the layer that owns both the outcome type AND the locale.

### `fl!()` numeric arg type: `usize` needs explicit `as i64` cast (Fluent i18n PR-3, #114)

`fl!()` numeric args must be `i64`. `usize` does not coerce automatically. Pattern: `let n = n as i64; fl!(loader, "msg-id", n = n)`. Applied to `page_unavailable`, `added_books`, and `added_books_save_failed` in `dynamic.rs`.

### `open_error` dual-form: `&dyn Display` for tests, `&str` for production (Fluent i18n PR-3, #114)

`open_error(loader, &dyn Display)` formats an error at call time and is only needed by tests that construct the message directly from an error value. Production always goes through `OpenOutcome::Error(String)` — the error was pre-captured as a `String` by `run()` — so the production path calls `open_error_str(loader, &str)` which takes the already-formatted string. The `&dyn Display` form is therefore `#[cfg(test)]`, keeping it out of the release binary and making the test/production split explicit and enforced by the compiler.

### Test guarantee migration discipline: named successors for every deleted test (Fluent i18n PR-3, #114)

When `messages.rs` was deleted its tests needed named successors in `i18n::dynamic::tests`. The mapping table in the PR description is the audit trail. Core obligations that MUST survive the deletion:
- "English historical strings" — byte-exact pins for messages that carry invisible whitespace or em dashes (e.g. ` (zip-slip or oversized)`, `—`). These pins are not redundant; they guard authoring regressions in the `.ftl`.
- "Cross-locale differ" — asserts `en_string != ja_string` per message family, proving the catalog is not English-only. A test that passes on a catalog that has only one locale is vacuous.
- "Args are embedded" — `assert!(formatted.contains(arg_value))` per parameterized function. Covers the `fl!()` arg-name wiring without byte-pinning translated text.

### First modifier-key arm (Cmd/Ctrl+A): verify Slint key delivery in the vendored source, don't assume (bulk-delete PR-4, #128)

`Carousel.slint`'s FocusScope gained the repo's FIRST `event.modifiers` usage — the select-all chord `if (root.selection-mode && (event.modifiers.control || event.modifiers.meta) && event.text == "a")`. Two delivery facts had to be confirmed in the vendored Slint source before trusting the arm; both were verified at the pinned 1.16.1:

1. **The chord delivers the plain LETTER, never the control character.** `i-slint-backend-winit-1.16.1/event_loop.rs` `to_slint_key` builds `event.text` from winit's `logical_key`: the match arm `winit::keyboard::Key::Character(str) => str.as_str().into()` means a modifier+letter chord (logical key `Character("a")`) arrives as `event.text == "a"` — the plain lowercase letter, NOT the ASCII control char `"\u{1}"`. So the arm tests `event.text == "a"`, not a control code.
2. **macOS modifier remap.** `i-slint-common-1.16.1/builtin_structs.rs` documents on `KeyboardModifiers` that on macOS Slint maps **Cmd → `control`** and **physical Ctrl → `meta`** (to make Cmd-based Apple shortcuts portable), while on Windows the Windows key maps to `meta` and Ctrl stays `control`. Therefore `event.modifiers.control || event.modifiers.meta` covers Cmd+A (macOS, `control`), physical Ctrl+A on macOS (`meta`), and Ctrl+A on Windows/Linux (`control`) in one arm.

HARNESS: when adding a keyboard chord, verify the key-delivery shape in the vendored source rather than assuming the control-character or a single-modifier form. Keep the arm selection-mode-gated (`root.selection-mode && …`) and ORDER it so a bare letter (`a` with no modifier never matches the modifier guard) and a normal-mode chord fall through to the FocusScope's terminal `return reject`. The existing `x` / Space selection arms (PR-2) sit above it; Esc/`/`/arrows/Return below — the chord slots between, after the modal-open reject guard.

### Two sanctioned ways to dodge the Slint preferred-width binding loop on a glass pill (bulk-delete PR-4, #128)

Two glass-pill components now demonstrate the two valid escapes from Slint's preferred-width binding-loop trap (a `width:` expression that reads the layout's own preferred width self-references):

- **NavBar** — a FIXED token-composed width formula (existing; see the NavBar header / width-formula material, not restated here). Correct when the chrome's footprint is fixed/known.
- **SelectionToolbar** — NO `width` binding at ALL, plus `horizontal-stretch: 0`, so the root resolves its preferred content size INTRINSICALLY from its `HorizontalLayout`. Correct for a content-HUGGING pill whose label length varies at runtime and per-locale (the Rust-composed `count-text` / `select-all-label`): there is no fixed formula to write, and the absence of any width expression that reads the layout's preferred width is precisely what avoids the loop. The `SelectionToolbar.slint` header documents this in contrast to NavBar.

### Eager push of Rust-composed feature strings when Slint flips the mode WITHOUT a Rust callback (bulk-delete PR-4, #128)

`Carousel.slint`'s `selection-mode` is a TWO-WAY-bound `in-out property <bool>` that the Slint side flips DIRECTLY with no Rust round-trip: the FocusScope `x` arm does `root.selection-mode = true`, the "Select" entry pill's TouchArea sets it `true`, and the toolbar/Esc paths set it `false`. Because Rust never gets an on-entry callback for the mode flip, the Rust-composed toolbar strings (`carousel-selection-count-text`, `carousel-select-all-label`) CANNOT be computed lazily on mode-entry — they must be EAGERLY maintained at every point where the selection set OR the visible projection changes. `push_selection_strings` (`main.rs`) is the per-feature chokepoint; verified call sites: `on_carousel_toggle_selection`, `on_carousel_cover_clicked`, `on_carousel_select_all`, `on_carousel_exit_selection`, the language-switch handler, and `refresh_library_carousel` (which covers the boot build, the debounced query callback, and the add path). It composes both strings via `i18n::dynamic::selection_count_text` / `select_all_label` (the `fl!()` `usize as i64` cast and word-order discipline are the existing `fl!()`/word-order entries above — not restated). This is the per-feature sibling of the `Strings`-global `apply()` push (see "The `Strings`-global push" entry): `apply()` flips the whole catalog on a language switch; `push_selection_strings` keeps one feature's two derived labels fresh between language switches. Failure mode prevented: a stale "N selected" that lies to the user after a toggle/query-change the mode-flip alone would never recompute.

### Desync-warn parity for index-taking carousel handlers — keep the single lookup seam, diagnose in the cold arm (bulk-delete PR-4, #128)

`on_carousel_open` set the precedent of warning on a carousel/library desync with `(index, visible_len, library_len)`. PR-4 extends that to `on_carousel_toggle_selection` and `on_carousel_cover_clicked`: both keep `visible_index_to_path(&library, &search, index)` as the SINGLE visible-index→path lookup seam (the same projection hop as the open handler — see "Single visible-index projection for filtered views"), and put the diagnostics in the COLD `else` arm via a fresh re-borrow (`search.borrow().visible_indices().len()` / `library.borrow().books().len()`) — safe because the helper's borrows have already dropped by the time the `let-else` enters the else. ANTI-PATTERN caught in review: inlining the helper back into each handler to capture the two lengths inside one borrow (as `on_carousel_open` does) would have left `visible_index_to_path` with a single remaining caller and tempted a `#[allow(dead_code)]` band-aid — but `allow(dead_code)` marks a symbol *not-yet-consumed*, never *no-longer-consumed*; the right move is the cold-arm re-borrow that keeps the seam shared and live.
